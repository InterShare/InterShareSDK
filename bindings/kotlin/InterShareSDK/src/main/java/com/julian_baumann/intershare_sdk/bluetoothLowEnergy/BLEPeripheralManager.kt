package com.julian_baumann.intershare_sdk.bluetoothLowEnergy

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.*
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.bluetooth.le.BluetoothLeAdvertiser
import android.content.Context
import android.content.pm.PackageManager
import android.os.ParcelUuid
import android.util.Log
import androidx.core.app.ActivityCompat
import com.julian_baumann.intershare_sdk.*
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import java.util.*

// Constants for optimized advertising
private const val ADVERTISING_RETRY_DELAY_MS = 1000L
private const val MAX_ADVERTISING_RETRIES = 3

class BlePermissionNotGrantedException : Exception()
val discoveryServiceUUID: UUID = UUID.fromString(getBleServiceUuid())
val discoveryCharacteristicUUID: UUID = UUID.fromString(getBleDiscoveryCharacteristicUuid())

internal class BLEPeripheralManager(private val context: Context, private val internalNearbyServer: InternalNearbyServer, private val bluetoothManager: BluetoothManager) : BleServerImplementationDelegate {

    private var bluetoothGattServer: BluetoothGattServer? = null
    private var bluetoothL2CAPServer: BluetoothServerSocket? = null
    private var l2CAPThread: Thread? = null
    @Volatile private var l2CAPRunning = false
    private var advertisingRetryCount = 0
    private val manufacturerId: Int = getBleManufacturerId().toInt()

    private fun createService(): BluetoothGattService {
        val service = BluetoothGattService(discoveryServiceUUID, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        val characteristic = BluetoothGattCharacteristic(discoveryCharacteristicUUID, BluetoothGattCharacteristic.PROPERTY_READ, BluetoothGattCharacteristic.PERMISSION_READ)

        service.addCharacteristic(characteristic)

        return service
    }

    private val gattServerCallback = object : BluetoothGattServerCallback() {
        override fun onCharacteristicReadRequest(
            device: BluetoothDevice?,
            requestId: Int,
            offset: Int,
            characteristic: BluetoothGattCharacteristic?
        ) {
            if (ActivityCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
                throw BlePermissionNotGrantedException()
            }

            CoroutineScope(Dispatchers.Main).launch {
                val data = internalNearbyServer.getAdvertisementData()

                bluetoothGattServer?.sendResponse(device,
                    requestId,
                    BluetoothGatt.GATT_SUCCESS,
                    0,
                    data
                )
            }
        }
    }

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartSuccess(settingsInEffect: AdvertiseSettings) {
            Log.i("InterShareSDK [BLE Manager]", "LE Advertise Started Successfully")
            advertisingRetryCount = 0 // Reset retry count on success
        }

        override fun onStartFailure(errorCode: Int) {
            Log.w("InterShareSDK [BLE Manager]", "LE Advertise Failed: $errorCode")
            
            // Retry advertising with exponential backoff
            if (advertisingRetryCount < MAX_ADVERTISING_RETRIES) {
                advertisingRetryCount++
                Log.d("InterShareSDK [BLE Manager]", "Retrying advertising attempt $advertisingRetryCount")
                
                CoroutineScope(Dispatchers.IO).launch {
                    kotlinx.coroutines.delay(ADVERTISING_RETRY_DELAY_MS * advertisingRetryCount)
                    startAdvertising()
                }
            } else {
                Log.e("InterShareSDK [BLE Manager]", "Failed to start advertising after $MAX_ADVERTISING_RETRIES attempts")
            }
        }
    }

    private fun startGattServer() {
        if (ActivityCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
            throw BlePermissionNotGrantedException()
        }

        val l2capServer = bluetoothManager.adapter.listenUsingInsecureL2capChannel()
        bluetoothL2CAPServer = l2capServer
        l2CAPRunning = true

        l2CAPThread = Thread {
            try {
                val psm = l2capServer.psm.toUInt()
                internalNearbyServer.setBluetoothLeDetails(BluetoothLeConnectionInfo("", psm))

                while (l2CAPRunning) {
                    val connection = l2capServer.accept()
                    val stream = L2CAPStream(connection)

                    CoroutineScope(Dispatchers.Main).launch {
                        internalNearbyServer.handleIncomingConnection(stream)
                    }
                }
            }
            catch (e: Exception) {
                // Expected when the server socket is closed on stop.
                if (l2CAPRunning) {
                    Log.e("InterShareSDK [BLE Manager]", e.toString())
                }
            }
        }

        l2CAPThread!!.start()

        bluetoothGattServer = bluetoothManager.openGattServer(context, gattServerCallback)
        bluetoothGattServer?.addService(createService())
            ?: Log.w("InterShareSDK [BLE Manager]", "Unable to create GATT server")
    }

    private fun stopGattServer() {
        if (ActivityCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
            throw BlePermissionNotGrantedException()
        }

        // Stop the accept loop and release the L2CAP server socket so the thread
        // exits and we don't leak a socket/thread on the next start.
        l2CAPRunning = false
        try {
            bluetoothL2CAPServer?.close()
        } catch (e: Exception) {
            Log.w("InterShareSDK [BLE Manager]", "Error closing L2CAP server: $e")
        }
        bluetoothL2CAPServer = null
        l2CAPThread?.interrupt()
        l2CAPThread = null

        bluetoothGattServer?.close()
        bluetoothGattServer = null
    }

    @SuppressLint("MissingPermission")
    private fun startAdvertising() {
        val bluetoothLeAdvertiser: BluetoothLeAdvertiser? = bluetoothManager.adapter.bluetoothLeAdvertiser

        bluetoothLeAdvertiser?.let {
            val settings = AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_LOW_LATENCY)
                .setConnectable(true)
                .setTimeout(0) // No timeout for continuous advertising
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_HIGH)
                .build()

            // Primary advertisement: just the service UUID. The 128-bit UUID
            // already consumes most of the 31-byte legacy budget, so the
            // correlation token goes in the scan response instead.
            val data = AdvertiseData.Builder()
                .setIncludeDeviceName(false)
                .addServiceUuid(ParcelUuid(discoveryServiceUUID))
                .build()

            // Scan response: the compact correlation token as manufacturer data.
            // This lets scanners track us across BLE MAC rotation without
            // reconnecting, and avoids mutating the device's global Bluetooth name.
            val scanResponseBuilder = AdvertiseData.Builder()
                .setIncludeDeviceName(false)
                .setIncludeTxPowerLevel(false)

            internalNearbyServer.getBleAdvertisementName()?.let { token ->
                scanResponseBuilder.addManufacturerData(manufacturerId, token.toByteArray(Charsets.UTF_8))
            }

            if (ActivityCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
                throw BlePermissionNotGrantedException()
            }

            it.startAdvertising(settings, data, scanResponseBuilder.build(), advertiseCallback)
        } ?: Log.w("InterShareSDK [BLE Manager]", "Failed to create advertiser")
    }

    private fun stopAdvertising() {
        if (ActivityCompat.checkSelfPermission(context, Manifest.permission.BLUETOOTH_ADVERTISE) != PackageManager.PERMISSION_GRANTED) {
            throw BlePermissionNotGrantedException()
        }

        val bluetoothLeAdvertiser: BluetoothLeAdvertiser? = bluetoothManager.adapter.bluetoothLeAdvertiser
        bluetoothLeAdvertiser?.stopAdvertising(advertiseCallback) ?: Log.w("InterShareSDK [BLE Manager]", "Failed to create advertiser")
    }

    override fun startServer() {
        if (!bluetoothManager.adapter.isEnabled) {
            Log.d("InterShareSDK [BLE Manager]", "Bluetooth is currently disabled...enabling")
        } else {
            Log.d("InterShareSDK [BLE Manager]", "Bluetooth enabled...starting optimized services")
            startGattServer()
            startAdvertising()
        }
    }

    override fun stopServer() {
        stopAdvertising()
        stopGattServer()
    }
}
