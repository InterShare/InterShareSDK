package com.julian_baumann.intershare_sdk

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import androidx.core.content.edit
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import kotlin.UByte

private const val STORE_NAME = "intershare_tls_store"
private const val KEY_IDENTITY_CERT = "identity_certificate"
private const val KEY_IDENTITY_KEY = "identity_private_key"
private const val LOG_TAG = "InterShareTLS"
private const val KEYSTORE_ALIAS = "intershare_tls_key"
private const val TRANSFORMATION = "AES/GCM/NoPadding"
private const val GCM_IV_SIZE = 12
private const val GCM_TAG_LENGTH = 128

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
    private val sharedPreferences =
        context.applicationContext.getSharedPreferences(STORE_NAME, Context.MODE_PRIVATE)
    private val keyStore: KeyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }

    override fun loadIdentity(): TlsIdentity? {
        val certificatePayload = sharedPreferences.getString(KEY_IDENTITY_CERT, null)
        val privateKeyPayload = sharedPreferences.getString(KEY_IDENTITY_KEY, null)

        if (certificatePayload == null || privateKeyPayload == null) {
            Log.d(LOG_TAG, "No cached TLS identity found")
            return null
        }

        val certificateBytes = decrypt(certificatePayload)
        val privateKeyBytes = decrypt(privateKeyPayload)

        if (certificateBytes == null || privateKeyBytes == null) {
            Log.e(LOG_TAG, "Failed to decrypt TLS identity")
            return null
        }

        Log.i(LOG_TAG, "Recovered TLS identity from secure storage")
        return TlsIdentity(
            certificateDer = certificateBytes.toUByteList(),
            privateKeyDer = privateKeyBytes.toUByteList()
        )
    }

    override fun storeIdentity(identity: TlsIdentity) {
        val certificatePayload = encrypt(identity.certificateDer.toByteArray())
        val privateKeyPayload = encrypt(identity.privateKeyDer.toByteArray())

        if (certificatePayload == null || privateKeyPayload == null) {
            Log.e(LOG_TAG, "Failed to encrypt TLS identity")
            return
        }

        sharedPreferences.edit {
            putString(KEY_IDENTITY_CERT, certificatePayload)
            putString(KEY_IDENTITY_KEY, privateKeyPayload)
        }
        Log.i(LOG_TAG, "Stored TLS identity")
    }

    override fun clearIdentity() {
        sharedPreferences.edit {
            remove(KEY_IDENTITY_CERT)
            remove(KEY_IDENTITY_KEY)
        }
        Log.i(LOG_TAG, "Cleared TLS identity")
    }

    override fun loadRemoteCertificate(deviceId: String): List<UByte>? {
        val payload = sharedPreferences.getString(remoteKey(deviceId), null) ?: return null
        val bytes = decrypt(payload)
        if (bytes == null) {
            Log.e(LOG_TAG, "Failed to decrypt fingerprint for device $deviceId")
            return null
        }
        Log.i(LOG_TAG, "Loaded certificate fingerprint for device $deviceId")
        return bytes.toUByteList()
    }

    override fun storeRemoteCertificate(deviceId: String, certificateDer: List<UByte>) {
        val payload = encrypt(certificateDer.toByteArray())
        if (payload == null) {
            Log.e(LOG_TAG, "Failed to encrypt fingerprint for device $deviceId")
            return
        }

        sharedPreferences.edit {
            putString(remoteKey(deviceId), payload)
        }
        Log.i(LOG_TAG, "Stored certificate fingerprint for device $deviceId")
    }

    override fun clearRemoteCertificate(deviceId: String) {
        sharedPreferences.edit {
            remove(remoteKey(deviceId))
        }
        Log.i(LOG_TAG, "Cleared certificate fingerprint for device $deviceId")
    }

    private fun remoteKey(deviceId: String) = "remote_$deviceId"

    private fun encrypt(plain: ByteArray): String? = try {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, getOrCreateSecretKey())
        val iv = cipher.iv
        val ciphertext = cipher.doFinal(plain)
        val combined = ByteArray(iv.size + ciphertext.size)
        System.arraycopy(iv, 0, combined, 0, iv.size)
        System.arraycopy(ciphertext, 0, combined, iv.size, ciphertext.size)
        Base64.encodeToString(combined, Base64.NO_WRAP)
    } catch (ex: Exception) {
        Log.e(LOG_TAG, "Encrypt error: ${ex.message}")
        null
    }

    private fun decrypt(payload: String): ByteArray? {
        return try {
            val combined = Base64.decode(payload, Base64.NO_WRAP)
            if (combined.size < GCM_IV_SIZE) {
                Log.e(LOG_TAG, "Ciphertext too short")
                return null
            }
            val iv = combined.copyOfRange(0, GCM_IV_SIZE)
            val ciphertext = combined.copyOfRange(GCM_IV_SIZE, combined.size)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            val spec = GCMParameterSpec(GCM_TAG_LENGTH, iv)
            cipher.init(Cipher.DECRYPT_MODE, getOrCreateSecretKey(), spec)
            cipher.doFinal(ciphertext)
        } catch (ex: Exception) {
            Log.e(LOG_TAG, "Decrypt error: ${ex.message}")
            null
        }
    }

    private fun getOrCreateSecretKey(): SecretKey {
        val existing = keyStore.getEntry(KEYSTORE_ALIAS, null) as? KeyStore.SecretKeyEntry
        if (existing != null) {
            return existing.secretKey
        }

        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        val spec = KeyGenParameterSpec.Builder(
            KEYSTORE_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(false)
            .build()

        generator.init(spec)
        return generator.generateKey()
    }

    private fun ByteArray.toUByteList(): List<UByte> = this.map { it.toUByte() }

    private fun List<UByte>.toByteArray(): ByteArray = this.map { it.toByte() }.toByteArray()
}
