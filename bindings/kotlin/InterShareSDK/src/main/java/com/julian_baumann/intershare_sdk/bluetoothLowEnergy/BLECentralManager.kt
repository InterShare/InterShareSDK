package com.julian_baumann.intershare_sdk.bluetoothLowEnergy

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.*
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.pm.PackageManager
import android.os.Handler
import android.os.Looper
import android.os.ParcelUuid
import android.util.Log
import androidx.core.app.ActivityCompat
import com.julian_baumann.intershare_sdk.BleDiscoveryImplementationDelegate
import com.julian_baumann.intershare_sdk.InternalDiscovery
import com.julian_baumann.intershare_sdk.getBleManufacturerId
import kotlinx.coroutines.*
import java.util.*
import java.util.concurrent.ConcurrentHashMap

// Scanning / connection tuning.
private const val MAX_CONCURRENT_CONNECTIONS = 5
private const val CONNECTION_TIMEOUT_MS = 8000L
private const val EXPIRY_INTERVAL_MS = 2000L
private const val DEVICE_TTL_SECONDS = 10UL
private const val SCAN_RESTART_DELAY_MS = 1000L
private const val PREFERRED_MTU = 150

@SuppressLint("MissingPermission")
class BLECentralManager(private val context: Context, private val internal: InternalDiscovery) : BleDiscoveryImplementationDelegate {
    private val bluetoothManager: BluetoothManager by lazy {
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    }
    private val manufacturerId: Int = getBleManufacturerId().toInt()
    private val mainHandler = Handler(Looper.getMainLooper())

    @Volatile
    private var isScanning = false
    private var expiryJob: Job? = null

    // address -> gatt. Mutated from binder threads (scan/gatt callbacks) and the
    // main thread, so it must be concurrency-safe.
    private val activeConnections = ConcurrentHashMap<String, BluetoothGatt>()
    private val connectionTimeouts = ConcurrentHashMap<String, Runnable>()

    private inner class GattCallback(private val address: String, private val token: String?) : BluetoothGattCallback() {
        override fun onConnectionStateChange(gatt: BluetoothGatt, status: Int, newState: Int) {
            when (newState) {
                BluetoothProfile.STATE_CONNECTED -> {
                    if (status == BluetoothGatt.GATT_SUCCESS) {
                        Log.d("InterShareSDK [BLE Central]", "Connected to $address")
                        gatt.requestMtu(PREFERRED_MTU)
                    } else {
                        Log.w("InterShareSDK [BLE Central]", "Connect to $address failed with status $status")
                        finishConnection(address)
                    }
                }
                BluetoothProfile.STATE_DISCONNECTED -> {
                    Log.d("InterShareSDK [BLE Central]", "Disconnected from $address")
                    finishConnection(address)
                }
            }
        }

        override fun onMtuChanged(gatt: BluetoothGatt?, mtu: Int, status: Int) {
            // Proceed regardless of whether the MTU negotiation succeeded; a low
            // MTU may truncate a large read, but stalling here would leak the
            // connection slot entirely.
            if (status != BluetoothGatt.GATT_SUCCESS) {
                Log.w("InterShareSDK [BLE Central]", "MTU change failed ($status), discovering services anyway")
            }
            gatt?.discoverServices()
        }

        override fun onServicesDiscovered(gatt: BluetoothGatt, status: Int) {
            if (status != BluetoothGatt.GATT_SUCCESS) {
                Log.w("InterShareSDK [BLE Central]", "Service discovery failed with status: $status")
                gatt.disconnect()
                return
            }

            val service = gatt.getService(discoveryServiceUUID)
            val characteristic = service?.getCharacteristic(discoveryCharacteristicUUID)

            if (characteristic != null) {
                gatt.readCharacteristic(characteristic)
            } else {
                Log.w("InterShareSDK [BLE Central]", "Discovery characteristic not found")
                gatt.disconnect()
            }
        }

        // Android < 13
        @Deprecated("Deprecated")
        override fun onCharacteristicRead(gatt: BluetoothGatt?, characteristic: BluetoothGattCharacteristic?, status: Int) {
            if (gatt != null && characteristic?.value != null) {
                handleCharacteristicData(characteristic.value, status, gatt)
            } else if (gatt != null) {
                gatt.disconnect()
            }
        }

        override fun onCharacteristicRead(gatt: BluetoothGatt, characteristic: BluetoothGattCharacteristic, value: ByteArray, status: Int) {
            handleCharacteristicData(value, status, gatt)
        }

        private fun handleCharacteristicData(data: ByteArray, status: Int, gatt: BluetoothGatt) {
            if (status == BluetoothGatt.GATT_SUCCESS) {
                Log.d("InterShareSDK [BLE Central]", "GATT read successful for $address")
                internal.parseDiscoveryMessage(data, address, token)
            } else {
                Log.w("InterShareSDK [BLE Central]", "GATT read failed with status: $status for $address")
            }
            // Done with this peer; disconnecting triggers cleanup via the
            // disconnect callback (and the timeout is a safety net).
            gatt.disconnect()
        }
    }

    override fun startScanning() {
        if (isScanning) {
            Log.d("InterShareSDK [BLE Central]", "Already scanning, ignoring start request")
            return
        }
        beginScan()
    }

    private fun beginScan() {
        if (ActivityCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_SCAN) != PackageManager.PERMISSION_GRANTED) {
            throw BlePermissionNotGrantedException()
        }

        val scanner = bluetoothManager.adapter?.bluetoothLeScanner
        if (scanner == null) {
            Log.w("InterShareSDK [BLE Central]", "Bluetooth LE scanner unavailable (adapter off?)")
            return
        }

        isScanning = true

        val scanFilter = listOf(
            ScanFilter.Builder()
                .setServiceUuid(ParcelUuid(discoveryServiceUUID))
                .build()
        )

        val settings = ScanSettings.Builder()
            .setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY)
            .setMatchMode(ScanSettings.MATCH_MODE_AGGRESSIVE)
            .setNumOfMatches(ScanSettings.MATCH_NUM_MAX_ADVERTISEMENT)
            .setReportDelay(0L)
            .build()

        Log.d("InterShareSDK [BLE Central]", "Starting continuous BLE scan")
        scanner.startScan(scanFilter, settings, leScanCallback)

        startExpirySweep()
    }

    override fun stopScanning() {
        if (ActivityCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_SCAN) != PackageManager.PERMISSION_GRANTED) {
            throw BlePermissionNotGrantedException()
        }

        Log.d("InterShareSDK [BLE Central]", "Stopping BLE scanning")
        isScanning = false

        expiryJob?.cancel()
        expiryJob = null

        bluetoothManager.adapter?.bluetoothLeScanner?.stopScan(leScanCallback)

        // Tear down any in-flight connections.
        for (address in activeConnections.keys.toList()) {
            finishConnection(address)
        }
    }

    private fun startExpirySweep() {
        expiryJob?.cancel()
        expiryJob = CoroutineScope(Dispatchers.Default).launch {
            while (isActive) {
                delay(EXPIRY_INTERVAL_MS)
                internal.expireDevices(DEVICE_TTL_SECONDS)
            }
        }
    }

    private fun scheduleConnectionTimeout(address: String) {
        cancelConnectionTimeout(address)
        val timeout = Runnable {
            Log.w("InterShareSDK [BLE Central]", "Connection to $address timed out, aborting")
            finishConnection(address)
        }
        connectionTimeouts[address] = timeout
        mainHandler.postDelayed(timeout, CONNECTION_TIMEOUT_MS)
    }

    private fun cancelConnectionTimeout(address: String) {
        connectionTimeouts.remove(address)?.let { mainHandler.removeCallbacks(it) }
    }

    private fun finishConnection(address: String) {
        cancelConnectionTimeout(address)
        activeConnections.remove(address)?.close()
    }

    private fun extractToken(result: ScanResult): String? {
        val record = result.scanRecord ?: return null

        // iOS/Windows peers advertise the token as the local name; Android peers
        // carry it in manufacturer-specific data to avoid mutating the global
        // adapter name.
        val name = record.deviceName
        if (!name.isNullOrEmpty()) {
            return name
        }

        val manufacturerData = record.getManufacturerSpecificData(manufacturerId)
        if (manufacturerData != null && manufacturerData.isNotEmpty()) {
            return String(manufacturerData, Charsets.UTF_8)
        }

        return null
    }

    private fun handleScanResult(result: ScanResult) {
        val device = result.device
        val address = device.address
        val token = extractToken(result)

        // Records the advertisement as a liveness heartbeat and decides whether a
        // GATT read is actually needed (false for already-resolved peers).
        if (!internal.shouldConnect(token, address)) {
            return
        }

        if (activeConnections.containsKey(address) || activeConnections.size >= MAX_CONCURRENT_CONNECTIONS) {
            return
        }

        // connectGatt must be issued on the main thread to avoid spurious
        // status-133 failures on many Android devices.
        mainHandler.post { connectTo(device, token) }
    }

    private fun connectTo(device: BluetoothDevice, token: String?) {
        val address = device.address
        if (activeConnections.containsKey(address) || activeConnections.size >= MAX_CONCURRENT_CONNECTIONS) {
            return
        }

        Log.d("InterShareSDK [BLE Central]", "Connecting to $address")
        val gatt = device.connectGatt(context, false, GattCallback(address, token), BluetoothDevice.TRANSPORT_LE)
        if (gatt != null) {
            activeConnections[address] = gatt
            scheduleConnectionTimeout(address)
        }
    }

    private val leScanCallback: ScanCallback = object : ScanCallback() {
        override fun onScanResult(callbackType: Int, result: ScanResult) {
            handleScanResult(result)
        }

        override fun onBatchScanResults(results: List<ScanResult>) {
            results.forEach { handleScanResult(it) }
        }

        override fun onScanFailed(errorCode: Int) {
            Log.e("InterShareSDK [BLE Central]", "Scan failed with error code: $errorCode")
            isScanning = false

            // Restart from a clean state after a short delay. Calling startScanning
            // directly is not enough on its own because the scanner needs to be
            // re-registered.
            mainHandler.postDelayed({
                if (!isScanning) {
                    try {
                        beginScan()
                    } catch (e: Exception) {
                        Log.e("InterShareSDK [BLE Central]", "Failed to restart scan: ${e.message}")
                    }
                }
            }, SCAN_RESTART_DELAY_MS)
        }
    }
}
