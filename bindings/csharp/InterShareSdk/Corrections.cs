namespace InterShareSdk;

public class NearbyServer(Device myDevice, NearbyConnectionDelegate? @delegate)
    : InternalNearbyServer(myDevice, _downloadsPath, @delegate)
{
    private static readonly string _downloadsPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        "Downloads"
    );
}

public interface IDiscoveryDelegate : DeviceListUpdateDelegate;

public class Discovery(IDiscoveryDelegate? @delegate) : InternalDiscovery(@delegate)
{
}
