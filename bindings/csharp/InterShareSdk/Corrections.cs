using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;

namespace InterShareSdk;

public class NearbyServer(Device myDevice, NearbyConnectionDelegate? @delegate)
    : InternalNearbyServer(myDevice, _downloadsPath, @delegate)
{
    private static readonly string _downloadsPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        "Downloads"
    );

    static NearbyServer()
    {
        CertificateStore.EnsureRegistered();
    }
}

public interface IDiscoveryDelegate : DeviceListUpdateDelegate;

public class Discovery(IDiscoveryDelegate? @delegate) : InternalDiscovery(@delegate)
{
    static Discovery()
    {
        CertificateStore.EnsureRegistered();
    }
}

internal static class CertificateStore
{
    private static readonly object Sync = new();
    private static bool _registered;

    public static void EnsureRegistered()
    {
        if (_registered)
        {
            return;
        }

        lock (Sync)
        {
            if (_registered)
            {
                return;
            }

            set_certificate_store_delegate(new ProtectedCertificateStore());
            _registered = true;
            Debug.WriteLine("[InterShareTLS] Certificate store delegate registered", "TLS");
        }
    }

    private sealed class ProtectedCertificateStore : CertificateStoreDelegate
    {
        private static readonly string StoreDirectory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "InterShare",
            "tls"
        );

        private static string IdentityCertificatePath => Path.Combine(StoreDirectory, "identity.cert");
        private static string IdentityKeyPath => Path.Combine(StoreDirectory, "identity.key");

        public TlsIdentity? LoadIdentity()
        {
            var certificate = ReadProtected(IdentityCertificatePath);
            var privateKey = ReadProtected(IdentityKeyPath);

            if (certificate == null || privateKey == null)
            {
                Debug.WriteLine("[InterShareTLS] No cached TLS identity found", "TLS");
                return null;
            }

            Debug.WriteLine("[InterShareTLS] Restored TLS identity from secure storage", "TLS");
            return new TlsIdentity(certificate, privateKey);
        }

        public void StoreIdentity(TlsIdentity identity)
        {
            Directory.CreateDirectory(StoreDirectory);
            WriteProtected(IdentityCertificatePath, identity.CertificateDer);
            WriteProtected(IdentityKeyPath, identity.PrivateKeyDer);
            Debug.WriteLine("[InterShareTLS] Stored TLS identity", "TLS");
        }

        public void ClearIdentity()
        {
            DeleteFile(IdentityCertificatePath);
            DeleteFile(IdentityKeyPath);
            Debug.WriteLine("[InterShareTLS] Cleared TLS identity", "TLS");
        }

        public byte[]? LoadRemoteCertificate(string deviceId)
        {
            var path = RemoteCertificatePath(deviceId);
            var data = ReadProtected(path);
            if (data != null)
            {
                Debug.WriteLine($"[InterShareTLS] Loaded fingerprint for device {deviceId}", "TLS");
            }
            return data;
        }

        public void StoreRemoteCertificate(string deviceId, byte[] certificateDer)
        {
            Directory.CreateDirectory(StoreDirectory);
            var path = RemoteCertificatePath(deviceId);
            WriteProtected(path, certificateDer);
            Debug.WriteLine($"[InterShareTLS] Stored fingerprint for device {deviceId}", "TLS");
        }

        public void ClearRemoteCertificate(string deviceId)
        {
            DeleteFile(RemoteCertificatePath(deviceId));
            Debug.WriteLine($"[InterShareTLS] Cleared fingerprint for device {deviceId}", "TLS");
        }

        private static string RemoteCertificatePath(string deviceId)
        {
            var fingerprint = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(deviceId)));
            return Path.Combine(StoreDirectory, $"remote_{fingerprint}.bin");
        }

        private static byte[]? ReadProtected(string path)
        {
            if (!File.Exists(path))
            {
                return null;
            }

            try
            {
                var data = File.ReadAllBytes(path);
                return ProtectedData.Unprotect(data, null, DataProtectionScope.CurrentUser);
            }
            catch (Exception ex)
            {
                Debug.WriteLine($"[InterShareTLS] Failed to read protected data at {path}: {ex.Message}", "TLS");
                return null;
            }
        }

        private static void WriteProtected(string path, byte[] data)
        {
            var protectedData = ProtectedData.Protect(data, null, DataProtectionScope.CurrentUser);
            File.WriteAllBytes(path, protectedData);
        }

        private static void DeleteFile(string path)
        {
            if (File.Exists(path))
            {
                File.Delete(path);
            }
        }
    }
}
