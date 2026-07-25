package com.taskveil.app

import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import java.security.KeyStore
import javax.crypto.SecretKey
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AndroidCapsuleStoreTest {
    private val namespace = "p0123456789abcdef0123456789abcdef"
    private val otherNamespace = "pfedcba9876543210fedcba9876543210"

    @Before
    fun installAndClear() {
        AndroidCapsuleStore.install(ApplicationProvider.getApplicationContext())
        AndroidCapsuleStore.delete(namespace, "active")
        AndroidCapsuleStore.delete(namespace, "pending")
        AndroidCapsuleStore.delete(namespace, "session-tokens")
        AndroidCapsuleStore.delete(otherNamespace, "active")
    }

    @After
    fun clear() {
        AndroidCapsuleStore.delete(namespace, "active")
        AndroidCapsuleStore.delete(namespace, "pending")
        AndroidCapsuleStore.delete(namespace, "session-tokens")
        AndroidCapsuleStore.delete(otherNamespace, "active")
    }

    @Test
    fun activeAndPendingRoundTripAndKeystoreKeyIsNonExportable() {
        val active = byteArrayOf(2, 1, 3, 3, 7)
        val pending = byteArrayOf(2, 9, 9, 9)
        val expectedActive = active.clone()
        val expectedPending = pending.clone()
        AndroidCapsuleStore.store(namespace, "active", active)
        AndroidCapsuleStore.store(namespace, "pending", pending)

        assertArrayEquals(expectedActive, AndroidCapsuleStore.load(namespace, "active"))
        assertArrayEquals(expectedPending, AndroidCapsuleStore.load(namespace, "pending"))

        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        val key = keyStore.getKey(
            "com.taskveil.app.local-capsule-seal.v1",
            null,
        ) as SecretKey
        assertNull("Android Keystore AES key must not be exportable", key.encoded)
    }

    @Test
    fun deletingOneProfileDoesNotDeleteAnotherProfilesSameSlot() {
        val first = byteArrayOf(1, 2, 3)
        val second = byteArrayOf(4, 5, 6)
        val expectedSecond = second.clone()
        AndroidCapsuleStore.store(namespace, "active", first)
        AndroidCapsuleStore.store(otherNamespace, "active", second)

        AndroidCapsuleStore.delete(namespace, "active")

        assertNull(AndroidCapsuleStore.load(namespace, "active"))
        assertArrayEquals(expectedSecond, AndroidCapsuleStore.load(otherNamespace, "active"))
    }

    @Test
    fun sessionTokenSetRoundTripsAndIsDeletedAsOneProtectedValue() {
        val tokens = """{"version":1,"access_token":"a","refresh_token":"r"}""".toByteArray()
        val expected = tokens.clone()

        AndroidCapsuleStore.store(namespace, "session-tokens", tokens)

        assertArrayEquals(
            expected,
            AndroidCapsuleStore.load(namespace, "session-tokens"),
        )
        AndroidCapsuleStore.delete(namespace, "session-tokens")
        assertNull(AndroidCapsuleStore.load(namespace, "session-tokens"))
    }
}
