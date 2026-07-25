package com.taskveil.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Production local-capsule sealer.
 *
 * The AES-256 key is generated inside Android Keystore and is never exported.
 * SharedPreferences stores only nonce + AES-GCM ciphertext/tag.
 */
object AndroidCapsuleStore {
    private const val KEYSTORE = "AndroidKeyStore"
    private const val KEY_ALIAS = "com.taskveil.app.local-capsule-seal.v1"
    private const val PREFERENCES = "taskveil_local_capsules_v1"
    private const val NONCE_LENGTH = 12
    private const val TAG_BITS = 128

    @Volatile
    private var applicationContext: Context? = null

    init {
        System.loadLibrary("taskveil_app_bridge")
    }

    @JvmStatic
    fun install(context: Context) {
        applicationContext = context.applicationContext
        nativeInstallContext()
    }

    @JvmStatic
    private external fun nativeInstallContext()

    @JvmStatic
    fun load(namespace: String, slot: String): ByteArray? {
        requireNamespace(namespace)
        requireSlot(slot)
        val encoded = preferences().getString(preferenceKey(namespace, slot), null) ?: return null
        val sealed = Base64.decode(encoded, Base64.NO_WRAP)
        require(sealed.size > NONCE_LENGTH + TAG_BITS / 8) { "invalid sealed capsule" }
        val nonce = sealed.copyOfRange(0, NONCE_LENGTH)
        val ciphertext = sealed.copyOfRange(NONCE_LENGTH, sealed.size)
        return Cipher.getInstance("AES/GCM/NoPadding").run {
            init(Cipher.DECRYPT_MODE, sealingKey(), GCMParameterSpec(TAG_BITS, nonce))
            updateAAD(aad(namespace, slot))
            doFinal(ciphertext)
        }
    }

    @JvmStatic
    fun store(namespace: String, slot: String, plaintext: ByteArray) {
        try {
            requireNamespace(namespace)
            requireSlot(slot)
            require(plaintext.isNotEmpty()) { "empty capsule" }
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.ENCRYPT_MODE, sealingKey())
            cipher.updateAAD(aad(namespace, slot))
            val ciphertext = cipher.doFinal(plaintext)
            val sealed = cipher.iv + ciphertext
            check(
                preferences().edit()
                    .putString(
                        preferenceKey(namespace, slot),
                        Base64.encodeToString(sealed, Base64.NO_WRAP),
                    )
                    .commit(),
            ) { "capsule persistence failed" }
        } finally {
            plaintext.fill(0)
        }
    }

    @JvmStatic
    fun delete(namespace: String, slot: String) {
        requireNamespace(namespace)
        requireSlot(slot)
        check(preferences().edit().remove(preferenceKey(namespace, slot)).commit()) {
            "capsule deletion failed"
        }
    }

    private fun sealingKey(): SecretKey {
        val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }

        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setKeySize(256)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setRandomizedEncryptionRequired(true)
                .setUserAuthenticationRequired(false)
                .build(),
        )
        return generator.generateKey()
    }

    private fun preferences() = requireNotNull(applicationContext) {
        "Android capsule store was not installed"
    }.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    private fun preferenceKey(namespace: String, slot: String) = "$namespace:$slot"

    private fun aad(namespace: String, slot: String) =
        "taskveil/android-local-capsule/v1/$namespace/$slot".toByteArray(Charsets.UTF_8)

    private fun requireNamespace(namespace: String) {
        require(namespace.matches(Regex("p[0-9a-f]{32}"))) { "invalid profile namespace" }
    }

    private fun requireSlot(slot: String) {
        require(
            slot == "active" ||
                slot == "pending" ||
                slot == "device-identity" ||
                slot == "session-tokens",
        ) {
            "invalid capsule slot"
        }
    }
}
