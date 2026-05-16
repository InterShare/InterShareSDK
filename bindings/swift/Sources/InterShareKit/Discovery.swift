//
//  Discovery.swift
//  
//
//  Created by Julian Baumann on 06.01.24.
//

import Foundation

public  protocol DiscoveryDelegate: DeviceListUpdateDelegate {
    func discoveryDidUpdateState(state: BluetoothState)
}

public class Discovery {
    private let internalHandler: InternalDiscovery
    private let bleImplementation: BLEClientManager
    
    public init(delegate: DiscoveryDelegate) throws {
        CertificateStore.ensureRegistered()
        internalHandler = try InternalDiscovery(delegate: delegate)
        bleImplementation = BLEClientManager(delegate: delegate, internalHandler: internalHandler)
        internalHandler.addBleImplementation(implementation: bleImplementation)
    }
    
    public func startScan() throws {
        try bleImplementation.ensureValidState()
        
        internalHandler.start()
    }
    
    public func getDevices() -> [Device] {
        return internalHandler.getDevices()
    }
    
    public func stopScan() throws {
        try bleImplementation.ensureValidState()
        
        internalHandler.stop()
    }
}
