# Navigate to the src/data_rct_ffi directory
Push-Location src/intershare_windows

# Build for x86_64-pc-windows-msvc
cargo build --lib --release --target x86_64-pc-windows-msvc
# Build for aarch64-pc-windows-msvc
# cargo build --lib --release --features sync --target aarch64-pc-windows-msvc

# Return to the previous directory
Pop-Location

# Run uniffi-bindgen for C# bindings generation
uniffi-bindgen-cs .\src\intershare_windows\src\intershare_sdk.udl --out-dir="bindings/csharp/InterShareSdk"
