package com.julian_baumann.intershare_sdk.bluetoothLowEnergy

import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.util.Log
import com.julian_baumann.intershare_sdk.InternalNearbyServer
import com.julian_baumann.intershare_sdk.L2CapDelegate
import com.julian_baumann.intershare_sdk.handleIncomingL2capConnection
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

class L2CAPClientManager(
    private val internalHandler: InternalNearbyServer,
    private val bluetoothManager: BluetoothManager
) : L2CapDelegate {
    @SuppressLint("MissingPermission")
    override fun openL2capConnection(connectionId: String, peripheralUuid: String, psm: UInt) {
        // Resolving + connecting an L2CAP socket is blocking and can throw, so run
        // it off the caller's thread and never let an exception escape — otherwise
        // the requesting side would hang until its connect timeout.
        CoroutineScope(Dispatchers.IO).launch {
            try {
                // `peripheralUuid` is the peer's BLE MAC address (the value passed
                // as the ble uuid during discovery). Resolve it via the adapter so
                // we don't depend on the scanner still holding the device object.
                val device = bluetoothManager.adapter?.getRemoteDevice(peripheralUuid)
                if (device == null) {
                    Log.e("InterShareSDK [L2CAP Client]", "Could not resolve device $peripheralUuid")
                    return@launch
                }

                val socket = device.createInsecureL2capChannel(psm.toInt())
                socket.connect()
                val stream = L2CAPStream(socket)

                handleIncomingL2capConnection(connectionId, stream)
            } catch (e: Exception) {
                Log.e("InterShareSDK [L2CAP Client]", "Failed to open L2CAP channel: $e")
            }
        }
    }
}
