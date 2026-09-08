//
//  File.swift
//
//
//  Created by Julian Baumann on 30.01.24.
//

import Foundation
import CoreBluetooth

struct ConnectionDetails {
    let connectionId: String
    let PSM: CBL2CAPPSM
}

public class L2CAPClient: NSObject, CBCentralManagerDelegate, CBPeripheralDelegate, L2CapDelegate {
    private let centralManager = CBCentralManager()
    private let internalHandler: InternalNearbyServer
    private var connections: [CBPeripheral: ConnectionDetails] = [:]
    private var streams: [L2CapStream] = []
    private var isPoweredOn = false

    // Requests that arrived before the central manager was powered on. They are
    // flushed once it becomes ready, so an early request isn't silently dropped.
    private var pendingRequests: [(connectionId: String, peripheralUuid: String, psm: UInt32)] = []

    init(internalHandler: InternalNearbyServer) {
        self.internalHandler = internalHandler

        super.init()

        centralManager.delegate = self
    }

    public func centralManagerDidUpdateState(_ central: CBCentralManager) {
        isPoweredOn = central.state == .poweredOn

        if isPoweredOn {
            let pending = pendingRequests
            pendingRequests.removeAll()
            for request in pending {
                openL2capConnection(connectionId: request.connectionId, peripheralUuid: request.peripheralUuid, psm: request.psm)
            }
        }
    }

    public func peripheral(_ peripheral: CBPeripheral, didOpen channel: CBL2CAPChannel?, error: Error?) {
        if let error = error {
            print("InterShareSDK [L2CAP Client]: Failed to open L2CAP channel: \(error.localizedDescription)")
            connections.removeValue(forKey: peripheral)
            centralManager.cancelPeripheralConnection(peripheral)
            return
        }

        guard let channel else {
            connections.removeValue(forKey: peripheral)
            return
        }

        let connectionDetails = connections[peripheral]

        guard let connectionDetails else {
            return
        }

        // The channel is open; we no longer need the pending connection entry.
        connections.removeValue(forKey: peripheral)

        let l2capStream = L2CapStream(channel: channel)
        streams.append(l2capStream)

        Task {
            await handleIncomingL2capConnection(connectionId: connectionDetails.connectionId, nativeStream: l2capStream)
        }
    }

    public func openL2capConnection(connectionId: String, peripheralUuid: String, psm: UInt32) {
        guard let uuid = UUID(uuidString: peripheralUuid) else {
            print("InterShareSDK [L2CAP Client]: Invalid peripheral UUID \(peripheralUuid)")
            return
        }

        // Wait for the central manager to be powered on, otherwise
        // retrievePeripherals returns nothing and the request is lost.
        guard isPoweredOn else {
            print("InterShareSDK [L2CAP Client]: Not powered on yet, queuing L2CAP request")
            pendingRequests.append((connectionId, peripheralUuid, psm))
            return
        }

        let peripherals = centralManager.retrievePeripherals(withIdentifiers: [uuid])

        guard let peripheral = peripherals.first else {
            print("InterShareSDK [L2CAP Client]: Couldn't locate peripheral \(peripheralUuid)")
            return
        }

        connections[peripheral] = ConnectionDetails(connectionId: connectionId, PSM: CBL2CAPPSM(psm))
        peripheral.delegate = self

        centralManager.connect(peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        let connectionDetails = connections[peripheral]

        guard let connectionDetails else {
            return
        }

        peripheral.openL2CAPChannel(connectionDetails.PSM)
    }

    public func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        print("InterShareSDK [L2CAP Client]: Failed to connect peripheral: \(error?.localizedDescription ?? "unknown")")
        connections.removeValue(forKey: peripheral)
    }

    public func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        connections.removeValue(forKey: peripheral)
    }
}
