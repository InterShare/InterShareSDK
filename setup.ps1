rustup target add \
    x86_64-pc-windows-msvc \
    aarch64-pc-windows-msvc

cargo install uniffi-bindgen-cs --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.8.3+v0.25.0

dotnet tool install -g csharpier