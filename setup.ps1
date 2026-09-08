rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc

cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.0+v0.31.0

dotnet tool install -g csharpier