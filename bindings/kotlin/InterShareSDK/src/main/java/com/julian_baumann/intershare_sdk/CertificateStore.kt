package com.julian_baumann.intershare_sdk

import android.content.Context
import android.util.Base64
import android.util.Log
import androidx.security.crypto.EncryptedSharedPreferences
import androidx.security.crypto.MasterKey
import kotlin.UByte

private const val STORE_NAME = "intershare_tls_store"
private const val KEY_IDENTITY_CERT = "identity_certificate"
private const val KEY_IDENTITY_KEY = "identity_private_key"
private const val LOG_TAG = "InterShareTLS"

internal object CertificateStoreManager {
    private var registered = false

    fun ensureRegistered(context: Context) {
        if (!registered) {
            val delegate = SecureCertificateStore(context.applicationContext)
            setCertificateStoreDelegate(delegate)
            registered = true
            Log.i(LOG_TAG, "Registered certificate store delegate")
        }
    }
}

private class SecureCertificateStore(context: Context) : CertificateStoreDelegate {
    private val sharedPreferences = createPreferences(context.applicationContext)

    private fun createPreferences(context: Context) = EncryptedSharedPreferences.create(
        context,
        STORE_NAME,
        MasterKey.Builder(context)
            .setKeyScheme(MasterKey.KeyScheme.AES256_GCM)
            .build(),
        EncryptedSharedPreferences.PrefKeyEncryptionScheme.AES256_SIV,
        EncryptedSharedPreferences.PrefValueEncryptionScheme.AES256_GCM
    )

    override fun loadIdentity(): TlsIdentity? {
        val certificateEncoded = sharedPreferences.getString(KEY_IDENTITY_CERT, null)
        val privateKeyEncoded = sharedPreferences.getString(KEY_IDENTITY_KEY, null)

        if (certificateEncoded == null || privateKeyEncoded == null) {
            Log.d(LOG_TAG, "No cached TLS identity found")
            return null
        }

        Log.i(LOG_TAG, "Recovered TLS identity from secure storage")
        return TlsIdentity(
            certificateDer = Base64.decode(certificateEncoded, Base64.NO_WRAP).toUByteList(),
            privateKeyDer = Base64.decode(privateKeyEncoded, Base64.NO_WRAP).toUByteList()
        )
    }

    override fun storeIdentity(identity: TlsIdentity) {
        sharedPreferences.edit()
            .putString(KEY_IDENTITY_CERT, identity.certificateDer.toByteArray().encode())
            .putString(KEY_IDENTITY_KEY, identity.privateKeyDer.toByteArray().encode())
            .apply()
        Log.i(LOG_TAG, "Stored TLS identity")
    }

    override fun clearIdentity() {
        sharedPreferences.edit()
            .remove(KEY_IDENTITY_CERT)
            .remove(KEY_IDENTITY_KEY)
            .apply()
        Log.i(LOG_TAG, "Cleared TLS identity")
    }

    override fun loadRemoteCertificate(deviceId: String): List<UByte>? {
        val encoded = sharedPreferences.getString(remoteKey(deviceId), null) ?: return null
        Log.i(LOG_TAG, "Loaded certificate fingerprint for device $deviceId")
        return Base64.decode(encoded, Base64.NO_WRAP).toUByteList()
    }

    override fun storeRemoteCertificate(deviceId: String, certificateDer: List<UByte>) {
        sharedPreferences.edit()
            .putString(remoteKey(deviceId), certificateDer.toByteArray().encode())
            .apply()
        Log.i(LOG_TAG, "Stored certificate fingerprint for device $deviceId")
    }

    override fun clearRemoteCertificate(deviceId: String) {
        sharedPreferences.edit()
            .remove(remoteKey(deviceId))
            .apply()
        Log.i(LOG_TAG, "Cleared certificate fingerprint for device $deviceId")
    }

    private fun remoteKey(deviceId: String) = "remote_$deviceId"

    private fun ByteArray.encode(): String = Base64.encodeToString(this, Base64.NO_WRAP)

    private fun ByteArray.toUByteList(): List<UByte> = this.map { it.toUByte() }

    private fun List<UByte>.toByteArray(): ByteArray = this.map { it.toByte() }.toByteArray()
}
