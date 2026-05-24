//
//  BleClient.swift
//
//
//  Created by Julian Baumann on 06.01.24.
//

import Foundation
import CoreBluetooth

public enum OpenL2CAPErrors: Error {
    case PeripheralNotFound
}

// Scanning / connection tuning.
private let MAX_CONCURRENT_CONNECTIONS = 5
private let CONNECTION_TIMEOUT: TimeInterval = 8.0   // Abort a stuck connect+read so its slot is freed.
private let EXPIRY_INTERVAL: TimeInterval = 2.0      // How often we sweep for departed devices.
private let DEVICE_TTL_SECONDS: UInt64 = 10          // Remove a peer not seen for this long.

public class BLEClientManager: NSObject, BleDiscoveryImplementationDelegate, CBCentralManagerDelegate, CBPeripheralDelegate {
    private let delegate: DiscoveryDelegate
    private let internalHandler: InternalDiscovery
    private let centralManager = CBCentralManager()
    private var state: BluetoothState = .unknown

    // Scan lifecycle. `shouldScan` is the desired state; actual scanning only
    // happens once Bluetooth is powered on, and resumes automatically if it
    // bounces off and back on.
    private var shouldScan = false

    // Peripherals we are currently connecting to / reading from. We must keep a
    // strong reference for the duration of the connection or CoreBluetooth will
    // silently drop it.
    private var activePeripherals: [UUID: CBPeripheral] = [:]
    private var peripheralTokens: [UUID: String] = [:]
    private var connectionTimeouts: [UUID: DispatchWorkItem] = [:]

    private var expiryTimer: Timer?

    init(delegate: DiscoveryDelegate, internalHandler: InternalDiscovery) {
        self.delegate = delegate
        self.internalHandler = internalHandler

        super.init()
        centralManager.delegate = self
    }

    public func ensureValidState() throws {
        if state != .poweredOn {
            throw InvalidStateError()
        }
    }

    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        state = BluetoothState(from: central.state)
        delegate.discoveryDidUpdateState(state: state)

        if state == .poweredOn {
            print("InterShareSDK [BLE Client]: Bluetooth is powered on")
            // Resume scanning if it was requested before powered-on, or after a
            // Bluetooth off -> on bounce.
            if shouldScan {
                beginScan()
            }
        } else {
            print("InterShareSDK [BLE Client]: Bluetooth state changed to: \(state)")
        }
    }

    public func startScanning() {
        shouldScan = true

        guard state == .poweredOn else {
            print("InterShareSDK [BLE Client]: Scan requested before powered on, deferring")
            return
        }

        beginScan()
    }

    private func beginScan() {
        if centralManager.isScanning {
            print("InterShareSDK [BLE Client]: Already scanning, ignoring start request")
            return
        }

        print("InterShareSDK [BLE Client]: Starting BLE scanning")

        // Allowing duplicates keeps a steady advertisement heartbeat, which is
        // what drives liveness/expiry in the SDK.
        centralManager.scanForPeripherals(withServices: [ServiceUUID], options: [
            CBCentralManagerScanOptionAllowDuplicatesKey: true
        ])

        startExpiryTimer()
    }

    public func stopScanning() {
        print("InterShareSDK [BLE Client]: Stopping BLE scanning")
        shouldScan = false

        stopExpiryTimer()

        for (identifier, peripheral) in activePeripherals {
            cancelConnectionTimeout(identifier)
            centralManager.cancelPeripheralConnection(peripheral)
        }
        activePeripherals.removeAll()
        peripheralTokens.removeAll()

        if centralManager.isScanning {
            centralManager.stopScan()
        }
        // NOTE: deliberately do NOT clear centralManager.delegate here — doing so
        // permanently breaks the manager and prevents any future scan from this
        // instance from receiving state updates or discovery callbacks.
    }

    // MARK: - Expiry

    private func startExpiryTimer() {
        stopExpiryTimer()
        expiryTimer = Timer.scheduledTimer(withTimeInterval: EXPIRY_INTERVAL, repeats: true) { [weak self] _ in
            self?.internalHandler.expireDevices(ttlSeconds: DEVICE_TTL_SECONDS)
        }
    }

    private func stopExpiryTimer() {
        expiryTimer?.invalidate()
        expiryTimer = nil
    }

    // MARK: - Connection timeout

    private func scheduleConnectionTimeout(_ peripheral: CBPeripheral) {
        let identifier = peripheral.identifier
        let work = DispatchWorkItem { [weak self] in
            guard let self = self else { return }
            print("InterShareSDK [BLE Client]: Connection to \(peripheral.name ?? "Unknown") timed out, aborting")
            self.centralManager.cancelPeripheralConnection(peripheral)
            self.cleanup(identifier)
        }
        connectionTimeouts[identifier] = work
        DispatchQueue.main.asyncAfter(deadline: .now() + CONNECTION_TIMEOUT, execute: work)
    }

    private func cancelConnectionTimeout(_ identifier: UUID) {
        connectionTimeouts[identifier]?.cancel()
        connectionTimeouts.removeValue(forKey: identifier)
    }

    private func cleanup(_ identifier: UUID) {
        cancelConnectionTimeout(identifier)
        activePeripherals.removeValue(forKey: identifier)
        peripheralTokens.removeValue(forKey: identifier)
    }

    // MARK: - Token extraction

    private func extractToken(from advertisementData: [String: Any]) -> String? {
        // iOS/macOS peers carry the token in the advertised local name.
        if let localName = advertisementData[CBAdvertisementDataLocalNameKey] as? String, !localName.isEmpty {
            return localName
        }

        // Android/Windows peers carry it in manufacturer-specific data, prefixed
        // with the little-endian company identifier.
        if let mfgData = advertisementData[CBAdvertisementDataManufacturerDataKey] as? Data, mfgData.count > 2 {
            let companyId = UInt16(mfgData[0]) | (UInt16(mfgData[1]) << 8)
            if companyId == getBleManufacturerId() {
                return String(data: mfgData.subdata(in: 2..<mfgData.count), encoding: .utf8)
            }
        }

        return nil
    }

    // MARK: - CBCentralManagerDelegate

    public func centralManager(_ central: CBCentralManager, didDiscover peripheral: CBPeripheral, advertisementData: [String : Any], rssi RSSI: NSNumber) {
        let token = extractToken(from: advertisementData)
        let identifier = peripheral.identifier

        // The SDK records the advertisement (liveness heartbeat) and tells us
        // whether a GATT read is actually needed. This returns false for peers
        // we have already resolved, which is what prevents reconnect storms.
        guard internalHandler.shouldConnect(token: token, deviceIdentifier: identifier.uuidString) else {
            return
        }

        // Already connecting to this peripheral, or at capacity.
        if activePeripherals[identifier] != nil {
            return
        }
        if activePeripherals.count >= MAX_CONCURRENT_CONNECTIONS {
            print("InterShareSDK [BLE Client]: At connection capacity, deferring \(peripheral.name ?? "Unknown")")
            return
        }

        activePeripherals[identifier] = peripheral
        if let token = token {
            peripheralTokens[identifier] = token
        }
        peripheral.delegate = self

        print("InterShareSDK [BLE Client]: Connecting to \(peripheral.name ?? "Unknown") (\(identifier))")
        scheduleConnectionTimeout(peripheral)
        central.connect(peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        print("InterShareSDK [BLE Client]: Connected to \(peripheral.name ?? "Unknown")")
        peripheral.discoverServices([ServiceUUID])
    }

    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        print("InterShareSDK [BLE Client]: Failed to connect to \(peripheral.name ?? "Unknown"): \(error?.localizedDescription ?? "Unknown error")")
        cleanup(peripheral.identifier)
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        print("InterShareSDK [BLE Client]: Disconnected from \(peripheral.name ?? "Unknown")")
        cleanup(peripheral.identifier)
    }

    // MARK: - CBPeripheralDelegate

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        if let error = error {
            print("InterShareSDK [BLE Client]: Service discovery failed for \(peripheral.name ?? "Unknown"): \(error.localizedDescription)")
            centralManager.cancelPeripheralConnection(peripheral)
            return
        }

        guard let service = peripheral.services?.first(where: { $0.uuid == ServiceUUID }) else {
            print("InterShareSDK [BLE Client]: Service not found for \(peripheral.name ?? "Unknown")")
            centralManager.cancelPeripheralConnection(peripheral)
            return
        }

        peripheral.discoverCharacteristics([DiscoveryCharacteristicUUID], for: service)
    }

    public func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        if let error = error {
            print("InterShareSDK [BLE Client]: Characteristic discovery failed for \(peripheral.name ?? "Unknown"): \(error.localizedDescription)")
            centralManager.cancelPeripheralConnection(peripheral)
            return
        }

        guard let characteristic = service.characteristics?.first(where: { $0.uuid == DiscoveryCharacteristicUUID }) else {
            print("InterShareSDK [BLE Client]: Discovery characteristic not found for \(peripheral.name ?? "Unknown")")
            centralManager.cancelPeripheralConnection(peripheral)
            return
        }

        peripheral.readValue(for: characteristic)
    }

    public func peripheral(_ peripheral: CBPeripheral, didModifyServices invalidatedServices: [CBService]) {
        // Re-resolve services if the peer changed its GATT database.
        if invalidatedServices.contains(where: { $0.uuid == ServiceUUID }) {
            peripheral.discoverServices([ServiceUUID])
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        if let error = error {
            print("InterShareSDK [BLE Client]: Characteristic read failed for \(peripheral.name ?? "Unknown"): \(error.localizedDescription)")
            centralManager.cancelPeripheralConnection(peripheral)
            return
        }

        if let data = characteristic.value {
            print("InterShareSDK [BLE Client]: Successfully read characteristic for \(peripheral.name ?? "Unknown")")
            let token = peripheralTokens[peripheral.identifier]
            internalHandler.parseDiscoveryMessage(data: data, bleUuid: peripheral.identifier.uuidString, token: token)
        } else {
            print("InterShareSDK [BLE Client]: No data received from characteristic for \(peripheral.name ?? "Unknown")")
        }

        // We have what we need; disconnect to free the link for the next peer.
        centralManager.cancelPeripheralConnection(peripheral)
    }
}
