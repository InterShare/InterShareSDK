package com.julian_baumann.intershare_sdk.bluetoothLowEnergy

import android.bluetooth.BluetoothSocket
import android.util.Log
import com.julian_baumann.intershare_sdk.NativeStreamDelegate

class L2CAPStream(private val socket: BluetoothSocket): NativeStreamDelegate {
    override fun write(data: ByteArray): ULong {
        try {
            socket.outputStream.write(data)

            return data.size.toULong()
        } catch (exception: Exception) {
            Log.w("InterShareSDK [L2CapStream]", "L2CAPStream write exception: $exception")

            return 0.toULong()
        }
    }

    override fun read(bufferLength: ULong): ByteArray {
        return try {
            val buffer = ByteArray(bufferLength.toInt())
            val readBytes = socket.inputStream.read(buffer)

            if (readBytes <= 0) {
                // <= 0 means end-of-stream (the peer closed the channel).
                ByteArray(0)
            } else {
                buffer.copyOfRange(0, readBytes)
            }
        } catch (exception: Exception) {
            // A closed/broken socket reads as EOF rather than crashing across the
            // FFI boundary.
            Log.w("InterShareSDK [L2CapStream]", "L2CAPStream read exception: $exception")
            ByteArray(0)
        }
    }

    override fun flush() {
        socket.outputStream.flush()
    }

    override fun disconnect() {
        socket.close()
    }
}
