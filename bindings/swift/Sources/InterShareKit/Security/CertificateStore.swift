import Foundation
import Security
import os.log

final class CertificateStore: CertificateStoreDelegate {
    static let shared = CertificateStore()
    private static var registered = false

    private let service = "app.intershare.tls"
    private let identityCertificateKey = "identity.certificate"
    private let identityPrivateKeyKey = "identity.privateKey"
    private let logger = OSLog(subsystem: "app.intershare.sdk", category: "CertificateStore")

    private init() {}

    static func ensureRegistered() {
        guard !registered else { return }
        setCertificateStoreDelegate(delegate: shared)
        os_log("Registered certificate store delegate", log: shared.logger, type: .info)
        registered = true
    }

    func loadIdentity() -> TlsIdentity? {
        guard
            let certificateData = read(key: identityCertificateKey),
            let privateKeyData = read(key: identityPrivateKeyKey)
        else {
            os_log("No TLS identity found in keychain", log: logger, type: .debug)
            return nil
        }

        os_log("Loaded TLS identity from keychain", log: logger, type: .info)
        return TlsIdentity(
            certificateDer: Array(certificateData),
            privateKeyDer: Array(privateKeyData)
        )
    }

    func storeIdentity(identity: TlsIdentity) {
        write(key: identityCertificateKey, data: Data(identity.certificateDer))
        write(key: identityPrivateKeyKey, data: Data(identity.privateKeyDer))
        os_log("Stored TLS identity in keychain", log: logger, type: .info)
    }

    func clearIdentity() {
        delete(key: identityCertificateKey)
        delete(key: identityPrivateKeyKey)
        os_log("Cleared TLS identity from keychain", log: logger, type: .info)
    }

    func loadRemoteCertificate(deviceId: String) -> [UInt8]? {
        guard let data = read(key: remoteKey(for: deviceId)) else { return nil }
        os_log("Loaded certificate fingerprint for device %{public}@", log: logger, type: .info, deviceId)
        return Array(data)
    }

    func storeRemoteCertificate(deviceId: String, certificateDer: [UInt8]) {
        write(key: remoteKey(for: deviceId), data: Data(certificateDer))
        os_log("Stored certificate fingerprint for device %{public}@", log: logger, type: .info, deviceId)
    }

    func clearRemoteCertificate(deviceId: String) {
        delete(key: remoteKey(for: deviceId))
        os_log("Removed certificate fingerprint for device %{public}@", log: logger, type: .info, deviceId)
    }

    private func remoteKey(for deviceId: String) -> String {
        "remote." + deviceId
    }

    private func read(key: String) -> Data? {
        var query = baseQuery(for: key)
        query[kSecReturnData as String] = kCFBooleanTrue
        query[kSecMatchLimit as String] = kSecMatchLimitOne

        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)

        guard status == errSecSuccess, let data = result as? Data else {
            if status != errSecItemNotFound {
                os_log(
                    "Failed to read keychain entry %{public}@ (status=%{public}d)",
                    log: logger,
                    type: .error,
                    key,
                    status
                )
            }
            return nil
        }

        return data
    }

    private func write(key: String, data: Data) {
        var attributes = baseQuery(for: key)
        attributes[kSecValueData as String] = data

        let status = SecItemAdd(attributes as CFDictionary, nil)
        if status == errSecDuplicateItem {
            let query = baseQuery(for: key)
            let update = [kSecValueData as String: data]
            SecItemUpdate(query as CFDictionary, update as CFDictionary)
        } else if status != errSecSuccess {
            os_log(
                "Failed to add keychain entry %{public}@ (status=%{public}d)",
                log: logger,
                type: .error,
                key,
                status
            )
        }
    }

    private func delete(key: String) {
        let query = baseQuery(for: key)
        let status = SecItemDelete(query as CFDictionary)
        if status != errSecSuccess && status != errSecItemNotFound {
            os_log(
                "Failed to delete keychain entry %{public}@ (status=%{public}d)",
                log: logger,
                type: .error,
                key,
                status
            )
        }
    }

    private func baseQuery(for key: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock
        ]
    }
}
