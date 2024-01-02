pub mod discovery;

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;

#[cfg(any(target_os = "android"))]
mod android;

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use apple::BleAdvertisement;

#[cfg(any(target_os = "android"))]
pub use android::BleAdvertisement;

pub const DISCOVERY_SERVICE_UUID: &str = "68D60EB2-8AAA-4D72-8851-BD6D64E169B7";
pub const DISCOVERY_CHARACTERISTIC_UUID: &str = "0BEBF3FE-9A5E-4ED1-8157-76281B3F0DA5";


#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;
    use protocol::discovery::{Device};
    use protocol::discovery::device::DeviceType;
    use crate::discovery::BleDiscovery;
    use crate::apple::BleAdvertisement;

    #[test]
    pub fn test_advertisement() {
        let my_device = Device {
            id: "43ED2550-3E5F-4ACC-BF58-DD0361A605C5".to_string(),
            name: "Test Device".to_string(),
            device_type: i32::from(DeviceType::Mobile)
        };

        let advertisement = BleAdvertisement::new(my_device);

        while !advertisement.is_powered_on() {}

        advertisement.start_advertising();

        thread::sleep(Duration::from_secs(20));
        advertisement.stop_advertising();
    }

    #[test]
    pub fn test_discovery() {
        let my_device = Device {
            id: "43ED2550-3E5F-4ACC-BF58-DD0361A605C5".to_string(),
            name: "Test Device".to_string(),
            device_type: i32::from(DeviceType::Mobile)
        };

        let discovery = BleDiscovery::new(None);
        discovery.start();
        // thread::sleep(Duration::from_secs(20));
        // discovery.stop();
        loop {}
    }

}
