pub use prost;

pub mod discovery {
    include!(concat!(env!("OUT_DIR"), "/inter_share_sdk.discovery.rs"));
}

pub mod communication {
    include!(concat!(
        env!("OUT_DIR"),
        "/inter_share_sdk.communication.rs"
    ));
}
