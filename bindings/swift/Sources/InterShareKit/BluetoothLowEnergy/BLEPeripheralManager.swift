//
//  File.swift
//  
//
//  Created by Julian Baumann on 05.01.24.
//

import Foundation
import CoreBluetooth

struct InvalidStateError: Error {}

// Constants for optimized advertising
private let ADVERTISING_RETRY_DELAY: TimeInterval = 1.0
private let MAX_ADVERTISING_RETRIES = 3

class BLEPeripheralManager: NSObject, BleServerImplementationDelegate, CBPeripheralManagerDelegate {
    private let peripheralManager: CBPeripheralManager
    private let internalHandler: InternalNearbyServer
    private let nearbyServerDelegate: NearbyServerDelegate
    private var streams: [L2CapStream] = []
    private var advertisingRetryCount = 0

    private var isPoweredOn = false
    // Desired advertising state. Lets us (re)start advertising automatically once
    // Bluetooth is powered on, including after an off -> on bounce.
    private var shouldAdvertise = false
    // Whether we have already published the L2CAP channel + service and started
    // advertising. Prevents repeated startServer() calls from publishing a new
    // L2CAP channel each time (which churns the PSM) and re-issuing advertising
    // (which fails with "Advertising has already started").
    private var isServing = false
    public var state: BluetoothState

    init(handler: InternalNearbyServer, delegate: NearbyServerDelegate) {
        nearbyServerDelegate = delegate
        internalHandler = handler
        peripheralManager = CBPeripheralManager()
        state = BluetoothState(from: peripheralManager.state)

        super.init()
        peripheralManager.delegate = self
    }

    func peripheralManagerDidUpdateState(_ peripheral: CBPeripheralManager) {
        state = BluetoothState(from: peripheral.state)
        nearbyServerDelegate.nearbyServerDidUpdateState(state: state)

        if state == .poweredOn {
            print("InterShareSDK [BLE Peripheral]: Bluetooth is powered on, ready for advertising")
            // CoreBluetooth tears down services and advertising when Bluetooth is
            // powered off. If we were meant to be advertising, re-establish it.
            if shouldAdvertise {
                beginServing()
            }
        } else {
            print("InterShareSDK [BLE Peripheral]: Bluetooth state changed to: \(state)")
            // CoreBluetooth drops the published channel/service and advertising
            // when not powered on; reflect that so we re-publish on the next
            // powered-on transition.
            isServing = false
        }
    }

    /// Publishes the L2CAP channel + service and starts advertising, but only
    /// once. Repeated calls while already serving are ignored.
    private func beginServing() {
        if isServing {
            print("InterShareSDK [BLE Peripheral]: Already serving, ignoring start request")
            return
        }
        isServing = true
        advertisingRetryCount = 0
        startL2CapServer()
    }
    
    public func ensureValidState() throws {
        if state != .poweredOn {
            throw InvalidStateError()
        }
    }
    
    func startL2CapServer() {
        peripheralManager.publishL2CAPChannel(withEncryption: false)
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager, didPublishL2CAPChannel PSM: CBL2CAPPSM, error: Error?) {
        print("L2CAP Channel PSM: \(PSM)")
        internalHandler.setBluetoothLeDetails(bleInfo: BluetoothLeConnectionInfo(uuid: "", psm: UInt32(PSM)))
        addService()
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager, didOpen channel: CBL2CAPChannel?, error: Error?) {
        print("L2CAP Channel was opened")
        
        guard let channel else {
            return
        }
        
        let l2capStream = L2CapStream(channel: channel)
        streams.append(l2capStream)

        Task {
            internalHandler.handleIncomingConnection(nativeStreamHandle: l2capStream)
        }
    }
    
    func addService() {
        let service = CBMutableService(type: ServiceUUID, primary: true)
        let discoveryCharacteristic = CBMutableCharacteristic(
            type: DiscoveryCharacteristicUUID,
            properties: [.read],
            value: nil,
            permissions: CBAttributePermissions.readable
        )

        let writeCharacteristic = CBMutableCharacteristic(
            type: WriteCharacteristicUUID,
            properties: [.write],
            value: nil,
            permissions: CBAttributePermissions.writeable
        )

        service.characteristics = [discoveryCharacteristic, writeCharacteristic]
        
        peripheralManager.add(service)
        // Do NOT start advertising here; wait for didAdd callback so the service is fully registered
    }
    
    private func startOptimizedAdvertising() {
        // Advertise the service UUID plus the compact correlation token as the
        // local name. The token lets scanners track this device across BLE MAC
        // rotation without reconnecting. (CBAdvertisementDataIsConnectable is not
        // an honored key for startAdvertising — service-backed advertising is
        // already connectable — so it is omitted.)
        var advertisingData: [String: Any] = [
            CBAdvertisementDataServiceUUIDsKey: [ServiceUUID]
        ]

        if let token = internalHandler.getBleAdvertisementName() {
            advertisingData[CBAdvertisementDataLocalNameKey] = token
        }

        print("InterShareSDK [BLE Peripheral]: Starting advertising")
        peripheralManager.startAdvertising(advertisingData)
    }
    
    private func retryAdvertising() {
        if advertisingRetryCount < MAX_ADVERTISING_RETRIES {
            advertisingRetryCount += 1
            print("InterShareSDK [BLE Peripheral]: Retrying advertising attempt \(advertisingRetryCount)")
            
            DispatchQueue.global().asyncAfter(deadline: .now() + ADVERTISING_RETRY_DELAY * Double(advertisingRetryCount)) { [weak self] in
                self?.startOptimizedAdvertising()
            }
        } else {
            print("InterShareSDK [BLE Peripheral]: Failed to start advertising after \(MAX_ADVERTISING_RETRIES) attempts")
        }
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager, didAdd service: CBService, error: Error?) {
        if let error = error {
            print("InterShareSDK [BLE Peripheral]: Failed to add service: \(error.localizedDescription)")
        } else {
            print("InterShareSDK [BLE Peripheral]: Service added successfully")
            // Start advertising only after the service has been added
            startOptimizedAdvertising()
        }
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveRead request: CBATTRequest) {
        Task {
            let data = await internalHandler.getAdvertisementData()

            // Honor the read offset so values larger than the ATT MTU are served
            // correctly across the blob-read requests CoreBluetooth issues. Returning
            // the full value on every request (ignoring offset) corrupts long reads.
            guard request.offset <= data.count else {
                peripheral.respond(to: request, withResult: .invalidOffset)
                return
            }

            request.value = data.subdata(in: request.offset..<data.count)
            peripheral.respond(to: request, withResult: .success)
        }
    }
    
    func peripheralManager(_ peripheral: CBPeripheralManager, didReceiveWrite requests: [CBATTRequest]) {
        // Handle write requests if needed
    }
    
    func peripheralManagerDidStartAdvertising(_ peripheral: CBPeripheralManager, error: Error?) {
        if let error = error {
            // "Advertising is already active" is not a real failure — advertising
            // is running, so don't retry (which would just fail again).
            if error.localizedDescription.localizedCaseInsensitiveContains("already") {
                print("InterShareSDK [BLE Peripheral]: Advertising already active, treating as started")
                advertisingRetryCount = 0
                return
            }

            print("InterShareSDK [BLE Peripheral]: Advertising failed: \(error.localizedDescription)")
            retryAdvertising()
        } else {
            print("InterShareSDK [BLE Peripheral]: Advertising started successfully")
            advertisingRetryCount = 0 // Reset retry count on success
        }
    }
    
    func startServer() {
        print("InterShareSDK [BLE Peripheral]: Starting server")
        shouldAdvertise = true

        guard state == .poweredOn else {
            print("InterShareSDK [BLE Peripheral]: Not powered on yet, will advertise once ready")
            return
        }

        beginServing()
    }

    func stopServer() {
        print("InterShareSDK [BLE Peripheral]: Stopping server")
        shouldAdvertise = false
        isServing = false
        peripheralManager.stopAdvertising()
        peripheralManager.removeAllServices()
    }
}
