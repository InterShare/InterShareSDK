package com.julian_baumann.intershare_sdk

import android.content.Context
import com.julian_baumann.intershare_sdk.bluetoothLowEnergy.BLECentralManager

interface DiscoveryDelegate: DeviceListUpdateDelegate

class Discovery(context: Context, delegate: DiscoveryDelegate) {
    init {
        CertificateStoreManager.ensureRegistered(context)
    }

    private val internal: InternalDiscovery = InternalDiscovery(delegate)
    private val bleImplementation: BLECentralManager = BLECentralManager(context, internal)

    init {
        internal.addBleImplementation(bleImplementation)
    }

    fun getDevices(): List<Device> {
        return internal.getDevices()
    }

    fun startScanning() {
        internal.start()
    }

    fun stopScanning() {
        internal.stop()
    }
}
