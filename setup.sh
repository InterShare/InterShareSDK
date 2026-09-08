#!/bin/bash
set -e

function setupApple() {
    echo "Setting up iOS and macOS targets"

    rustup target add \
        aarch64-apple-ios \
        aarch64-apple-ios-sim \
        x86_64-apple-ios \
        x86_64-apple-darwin \
        aarch64-apple-darwin
}

function setupAndroid() {
    echo "Setting up Android targets"

    rustup target add x86_64-linux-android \
        x86_64-unknown-linux-gnu \
        aarch64-linux-android \
        armv7-linux-androideabi \
        i686-linux-android
}

setupApple
setupAndroid
