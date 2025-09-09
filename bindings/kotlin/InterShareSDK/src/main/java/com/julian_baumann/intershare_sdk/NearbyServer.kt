package com.julian_baumann.intershare_sdk

import android.bluetooth.BluetoothManager
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Environment
import android.util.Log
import com.julian_baumann.intershare_sdk.bluetoothLowEnergy.BLEPeripheralManager
import com.julian_baumann.intershare_sdk.bluetoothLowEnergy.L2CAPClientManager
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

class NearbyServer(context: Context, myDevice: Device, delegate: NearbyConnectionDelegate) {
    private val bluetoothManager: BluetoothManager by lazy {
        context.getSystemService(Context.BLUETOOTH_SERVICE) as BluetoothManager
    }

    private val internal: InternalNearbyServer = InternalNearbyServer(
        myDevice,
        Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS).absolutePath,
        delegate
    )

    private val internalBleImplementation = BLEPeripheralManager(context, internal, bluetoothManager)
    private val internalL2CapClient = L2CAPClientManager(internal)
    private var currentIPAddress: String? = null
    private var connectivityManager: ConnectivityManager = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private var started = false

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            super.onAvailable(network)
            checkNetworkType()
        }

        override fun onLost(network: Network) {
            super.onLost(network)
            Log.d("InterShareSDK [NetworkMonitor]", "No network connection")
            currentIPAddress = null
        }
    }

    private fun checkNetworkType() {
        val activeNetwork: Network? = connectivityManager.activeNetwork
        val networkCapabilities: NetworkCapabilities? = connectivityManager.getNetworkCapabilities(activeNetwork)

        if (networkCapabilities != null) {
            val ip = internal.getCurrentIp()

            when {
                networkCapabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> {
                    Log.d("InterShareSDK [NetworkMonitor]", "Connected via Wi-Fi")
                    if (ip != currentIPAddress) {
                        currentIPAddress = ip
                        Log.d("InterShareSDK [NetworkMonitor]", "Wi-Fi IP Address: $ip")

                        if (started) {
                            CoroutineScope(Dispatchers.Default).launch {
                                internal.restartServer()
                            }
                        }
                    }
                }
                networkCapabilities.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> {
                    Log.d("InterShareSDK [NetworkMonitor]", "Connected via Cellular Data")
                    if (ip != currentIPAddress) {
                        currentIPAddress = ip
                        Log.d("InterShareSDK [NetworkMonitor]", "Cellular IP Address: $ip")

                        if (started) {
                            CoroutineScope(Dispatchers.Default).launch {
                                internal.restartServer()
                            }
                        }
                    }
                }
                else -> {
                    Log.d("InterShareSDK [NetworkMonitor]", "Unknown network type")
                    currentIPAddress = null
                }
            }
        }
    }

    val bluetoothEnabled: Boolean get() {
        return bluetoothManager.adapter.isEnabled
    }

    init {
        setTmpDir(context.cacheDir.absolutePath)

        internal.addBluetoothImplementation(internalBleImplementation)
        internal.addL2CapClient(internalL2CapClient)

        val request = NetworkRequest.Builder().build()
        connectivityManager.registerNetworkCallback(request, networkCallback)
    }

    suspend fun start() {
        internal.start()
        started = true
    }

    fun changeDevice(newDevice: Device) {
        internal.changeDevice(newDevice)
    }

    suspend fun shareFiles(urls: List<String>, allowConvenienceDownload: Boolean): ShareStore {
        return internal.shareFiles(urls, allowConvenienceDownload)
    }

    suspend fun shareText(text: String, allowConvenienceDownload: Boolean): ShareStore {
        return internal.shareText(text, allowConvenienceDownload)
    }

    suspend fun requestDownload(link: String) {
        return internal.requestDownload(link)
    }

    suspend fun stop() {
        internal.stop()
        started = false
    }
}
