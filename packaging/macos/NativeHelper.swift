// Native macOS adapter. All network authentication, routing and fail-open input
// leases remain in Rust. This helper owns only the OS resources of one worker.
import AppKit
import AVFoundation
import AudioToolbox
import ApplicationServices
import CoreAudio
import CoreGraphics
import CoreMedia
import CryptoKit
import Foundation
import IOKit
import ScreenCaptureKit
import Darwin

enum NativeError: Error, CustomStringConvertible {
    case message(String)
    var description: String { switch self { case .message(let value): return value } }
}
func fail(_ message: String) -> NativeError { .message(message) }
func checked(_ status: OSStatus, _ operation: String) throws {
    if status != noErr { throw fail("\(operation) failed (CoreAudio \(status))") }
}
func json(_ object: Any, to handle: FileHandle = .standardOutput) {
    guard let data = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else { return }
    handle.write(data + Data([10]))
}
func ready(_ name: String) { json(["ready": name], to: .standardError) }
func property(_ selector: AudioObjectPropertySelector, _ scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal, _ element: AudioObjectPropertyElement = kAudioObjectPropertyElementMain) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: element)
}
func defaultDevice(_ input: Bool) throws -> AudioDeviceID {
    var address = property(input ? kAudioHardwarePropertyDefaultInputDevice : kAudioHardwarePropertyDefaultOutputDevice)
    var device = AudioDeviceID(0), size = UInt32(MemoryLayout<AudioDeviceID>.size)
    try checked(AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &device), "Reading the default audio device")
    guard device != kAudioObjectUnknown else { throw fail("No default audio device is available") }
    return device
}
func stringProperty(_ device: AudioDeviceID, _ selector: AudioObjectPropertySelector) throws -> String {
    var address = property(selector)
    var value: CFString = "" as CFString
    var size = UInt32(MemoryLayout<CFString>.size)
    try checked(AudioObjectGetPropertyData(device, &address, 0, nil, &size, &value), "Reading audio device metadata")
    return value as String
}
func deviceIDs() throws -> [AudioDeviceID] {
    var address = property(kAudioHardwarePropertyDevices), size = UInt32(0)
    try checked(AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size), "Listing audio devices")
    guard size <= 16384 else { throw fail("Too many CoreAudio devices") }
    var values = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
    if values.isEmpty {return []}
    try values.withUnsafeMutableBytes { bytes in
        try checked(AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, bytes.baseAddress!), "Listing audio devices")
    }
    return values
}
func hasStreams(_ device: AudioDeviceID, _ input: Bool) -> Bool {
    var address = property(kAudioDevicePropertyStreams, input ? kAudioDevicePropertyScopeInput : kAudioDevicePropertyScopeOutput), size = UInt32(0)
    return AudioObjectGetPropertyDataSize(device, &address, 0, nil, &size) == noErr && size > 0
}
func device(_ uid: String?, input: Bool) throws -> AudioDeviceID {
    guard let uid = uid, !uid.isEmpty else { return try defaultDevice(input) }
    for candidate in try deviceIDs() where (try? stringProperty(candidate, kAudioDevicePropertyDeviceUID)) == uid && hasStreams(candidate, input) { return candidate }
    throw fail("The selected audio device is unavailable: \(uid)")
}
func cable(_ name: String) -> Bool {
    let value = name.lowercased()
    return value.contains("vb-cable") || value.contains("vb cable") || value.contains("blackhole") || value.contains("cable input") || value.contains("cable output")
}
func devices() throws {
    let defaultOutput = try? defaultDevice(false), defaultInput = try? defaultDevice(true)
    var outputs = [[String: Any]](), microphones = [[String: Any]](), bridges = [[String: Any]]()
    for id in try deviceIDs() {
        let uid = try stringProperty(id, kAudioDevicePropertyDeviceUID)
        let name = try stringProperty(id, kAudioObjectPropertyName)
        let item: [String: Any] = ["id": uid, "name": name, "default": id == defaultOutput]
        if hasStreams(id, false) { if cable(name) { bridges.append(item) } else { outputs.append(item) } }
        if hasStreams(id, true) && !cable(name) { microphones.append(["id": uid, "name": name, "default": id == defaultInput]) }
    }
    json(["speakers": outputs, "microphones": microphones, "bridges": bridges])
}

func inputDevices() throws {
    var iterator = io_iterator_t(0)
    guard IOServiceGetMatchingServices(kIOMainPortDefault, IOServiceMatching("IOHIDDevice"), &iterator) == KERN_SUCCESS else { throw fail("Cannot enumerate macOS HID devices") }
    defer { IOObjectRelease(iterator) }
    var keyboards = [String](), mice = [String]()
    while true {
        let entry = IOIteratorNext(iterator); if entry == 0 { break }; defer { IOObjectRelease(entry) }
        var properties: Unmanaged<CFMutableDictionary>?
        guard IORegistryEntryCreateCFProperties(entry, &properties, kCFAllocatorDefault, 0) == KERN_SUCCESS,
              let values = properties?.takeRetainedValue() as? [String: Any] else { continue }
        let name = values["Product"] as? String ?? "Attached HID device"
        if (values["Transport"] as? String)?.lowercased() == "virtual" || name.lowercased().contains("desk switch") { continue }
        var pairs = values["DeviceUsagePairs"] as? [[String: Any]] ?? []
        pairs.append(["DeviceUsagePage": values["PrimaryUsagePage"] ?? 0, "DeviceUsage": values["PrimaryUsage"] ?? 0])
        for pair in pairs where (pair["DeviceUsagePage"] as? Int) == 1 {
            if (pair["DeviceUsage"] as? Int) == 6 { keyboards.append(name) }
            if [1, 2].contains(pair["DeviceUsage"] as? Int ?? 0) { mice.append(name) }
        }
    }
    json(["keyboards": Array(Set(keyboards)).sorted(), "mice": Array(Set(mice)).sorted()])
}

func volumeAddress(_ id: AudioDeviceID, _ selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress? {
    // Master volume is preferred; channel controls are used only when the device
    // has no master control. Digital fixed-level outputs report full scale.
    for channel: UInt32 in [0, 1] {
        var address = property(selector, kAudioDevicePropertyScopeOutput, channel)
        if AudioObjectHasProperty(id, &address) { return address }
    }
    return nil
}
func volume(_ uid: String?, set: [String]) throws {
    let id = try device(uid, input: false)
    if set.count >= 1 && set[0] != "-" {
        guard let percent = Float32(set[0]), (0...100).contains(percent), var address = volumeAddress(id, kAudioDevicePropertyVolumeScalar) else { throw fail("This output has no adjustable macOS volume control") }
        var value = percent / 100
        try checked(AudioObjectSetPropertyData(id, &address, 0, nil, UInt32(MemoryLayout<Float32>.size), &value), "Setting output volume")
        if address.mElement == 1 {
            address.mElement = 2
            if AudioObjectHasProperty(id, &address) { try checked(AudioObjectSetPropertyData(id, &address, 0, nil, UInt32(MemoryLayout<Float32>.size), &value), "Setting the right output channel") }
        }
    }
    if set.count >= 2 && set[1] != "-" {
        guard ["true", "false"].contains(set[1]), var address = volumeAddress(id, kAudioDevicePropertyMute) else { throw fail("This output has no macOS mute control") }
        var value = UInt32(set[1] == "true" ? 1 : 0)
        try checked(AudioObjectSetPropertyData(id, &address, 0, nil, UInt32(MemoryLayout<UInt32>.size), &value), "Setting output mute")
        if address.mElement==1 {address.mElement=2;if AudioObjectHasProperty(id,&address){try checked(AudioObjectSetPropertyData(id,&address,0,nil,UInt32(MemoryLayout<UInt32>.size),&value),"Setting the right output mute")}}
    }
    var level = Float32(1), muted = UInt32(0)
    let fixed = volumeAddress(id, kAudioDevicePropertyVolumeScalar) == nil
    if var address = volumeAddress(id, kAudioDevicePropertyVolumeScalar) {
        var size = UInt32(MemoryLayout<Float32>.size)
        try checked(AudioObjectGetPropertyData(id, &address, 0, nil, &size, &level), "Reading output volume")
    }
    if var address = volumeAddress(id, kAudioDevicePropertyMute) {
        var size = UInt32(MemoryLayout<UInt32>.size)
        try checked(AudioObjectGetPropertyData(id, &address, 0, nil, &size, &muted), "Reading output mute")
    }
    json(["volume": Int((min(max(level, 0), 1) * 100).rounded()), "muted": muted != 0, "fixed": fixed])
}

func displays() throws {
    var count = UInt32(0)
    guard CGGetOnlineDisplayList(0, nil, &count) == .success, count <= 128 else { throw fail("Cannot enumerate macOS displays") }
    var values = [CGDirectDisplayID](repeating: 0, count: Int(count))
    guard CGGetOnlineDisplayList(count, &values, &count) == .success else { throw fail("Cannot enumerate macOS displays") }
    var result = [[String: Any]]()
    for id in values {
        let name = NSScreen.screens.first { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == id }?.localizedName ?? "Display \(id)"
        let uuid = CGDisplayCreateUUIDFromDisplayID(id).takeRetainedValue()
        let identity = CFUUIDCreateString(nil, uuid)! as String
        result.append(["id": "mac-display:\(id):\(identity)", "name": name, "screen": String(id), "fingerprint": "\(CGDisplayVendorNumber(id)):\(CGDisplayModelNumber(id)):\(CGDisplaySerialNumber(id))", "builtin": CGDisplayIsBuiltin(id) != 0])
    }
    json(result)
}

struct DesktopScreen: Equatable {
    let id: String, rect: CGRect, epoch: UInt64
    var value: [String:Any] { ["id":id,"x":rect.minX,"y":rect.minY,"width":rect.width,"height":rect.height,"epoch":NSNumber(value:epoch)] }
}
func desktopScreens() throws -> [DesktopScreen] {
    var count=UInt32(0)
    guard CGGetActiveDisplayList(0,nil,&count) == .success, count<=128 else {throw fail("Cannot read macOS screen geometry")}
    var displays=[CGDirectDisplayID](repeating:0,count:Int(count))
    guard CGGetActiveDisplayList(count,&displays,&count) == .success else {throw fail("Cannot read macOS screen geometry")}
    displays=Array(displays.prefix(Int(count))).sorted()
    var topology=[[String:Any]]()
    for id in displays {
        let bounds=CGDisplayBounds(id)
        guard [bounds.minX,bounds.minY,bounds.width,bounds.height].allSatisfy({$0.isFinite && $0.rounded() == $0}),bounds.width>=2,bounds.height>=2 else {throw fail("Invalid macOS screen geometry")}
        let uuid=CFUUIDCreateString(nil,CGDisplayCreateUUIDFromDisplayID(id).takeRetainedValue())! as String
        topology.append(["id":String(id),"uuid":uuid,"x":bounds.minX,"y":bounds.minY,"width":bounds.width,"height":bounds.height,"pixels_width":CGDisplayPixelsWide(id),"pixels_height":CGDisplayPixelsHigh(id),"rotation":CGDisplayRotation(id),"mirror":CGDisplayMirrorsDisplay(id)])
    }
    let data=try JSONSerialization.data(withJSONObject:topology,options:[.sortedKeys])
    let digest=SHA256.hash(data:Data("Desk Switch macOS desktop v1\0".utf8)+data)
    let epoch=max(1,digest.prefix(8).reduce(UInt64(0)) {($0<<8)|UInt64($1)})
    return displays.map {DesktopScreen(id:String($0),rect:CGDisplayBounds($0),epoch:epoch)}
}
func screenGeometryValue() throws -> [[String:Any]] {try desktopScreens().map(\.value)}

struct RouteRect: Decodable, Equatable {
    let x: Int32, y: Int32, width: UInt32, height: UInt32
    var rect: CGRect {CGRect(x:Double(x),y:Double(y),width:Double(width),height:Double(height))}
}
struct RouteBoundary: Decodable, Equatable {
    let id:UInt64, screen:RouteRect, geometry:UInt64, side:String, start:UInt16, end:UInt16
    func position(_ point:CGPoint,_ dx:Double,_ dy:Double)->UInt16? {
        let rect=screen.rect
        guard rect.contains(point) else {return nil}
        let t:Double
        switch side {
        case "left" where dx<0:t=(rect.minX-point.x)/dx
        case "right" where dx>0:t=(rect.maxX-1-point.x)/dx
        case "top" where dy<0:t=(rect.minY-point.y)/dy
        case "bottom" where dy>0:t=(rect.maxY-1-point.y)/dy
        default:return nil
        }
        guard t<=1 else {return nil}
        let vertical=side=="left" || side=="right"
        let offset=vertical ? point.y+max(0,t)*dy-rect.minY : point.x+max(0,t)*dx-rect.minX
        let span=vertical ? rect.height : rect.width
        guard offset>=0,offset<span else {return nil}
        let position=UInt16((min(1,offset/max(1,span-1))*65535).rounded())
        return position>=start && (position<end || end==UInt16.max && position==UInt16.max) ? position : nil
    }
}
struct RoutePolicy: Decodable, Equatable {
    var dwell_ms:UInt16=150, protect_corners=true, paused=false
    func protects(_ boundary:RouteBoundary,_ position:UInt16)->Bool {
        guard protect_corners else {return false}
        let rect=boundary.screen.rect
        let span=(boundary.side=="left" || boundary.side=="right" ? rect.height : rect.width)-1
        let corner=min(rect.width,rect.height)*0.025,offset=Double(position)/65535*span
        return offset<corner || span-offset<corner
    }
}
struct RouteDwell {
    var boundary:RouteBoundary, since:TimeInterval, last:TimeInterval
}
final class CaptureRoutes {
    var enabled=false, revision:UInt64=0, activation:UInt64=0
    var staged=[RouteBoundary](), boundaries=[RouteBoundary](), screens=[DesktopScreen]()
    var indexed=[String:[RouteBoundary]](), latch:(CGRect,String)?, cooldown:TimeInterval=0
    var policy=RoutePolicy(), dwell:RouteDwell?
    var emit:([String:Any])->Void={json($0)}
    var latched:Bool {if case .some=latch {return true};return false}
    func refresh(_ value:[DesktopScreen]) {
        if screens != value {latch=nil;cancelDwell()}
        screens=value;indexed.removeAll()
        for boundary in boundaries {
            let matches=value.filter {$0.rect==boundary.screen.rect && $0.epoch==boundary.geometry}
            if matches.count==1 {indexed[matches[0].id,default:[]].append(boundary)}
        }
    }
    func cancelDwell() {dwell=nil}
    func resetEntry() {latch=nil;cancelDwell();cooldown=ProcessInfo.processInfo.systemUptime+0.12}
    func command(_ value:[String:Any]) {
        switch value["kind"] as? String {
        case "routes-start":cancelDwell();staged.removeAll();boundaries.removeAll();indexed.removeAll()
        case "routes-add":
            if staged.count<4096,let object=value["boundary"],let data=try? JSONSerialization.data(withJSONObject:object),let boundary=try? JSONDecoder().decode(RouteBoundary.self,from:data),boundary.geometry != 0,boundary.start<boundary.end,boundary.screen.width>=2,boundary.screen.height>=2,["left","right","top","bottom"].contains(boundary.side) {staged.append(boundary)}
        case "routes-commit":
            guard let nextRevision=(value["revision"] as? NSNumber)?.uint64Value,let nextActivation=(value["activation"] as? NSNumber)?.uint64Value,nextRevision != 0 || staged.isEmpty else {return}
            var nextPolicy=RoutePolicy()
            if let object=value["policy"] {
                guard let data=try? JSONSerialization.data(withJSONObject:object),let decoded=try? JSONDecoder().decode(RoutePolicy.self,from:data),decoded.dwell_ms<=1000 else {return}
                nextPolicy=decoded
            }
            if activation != nextActivation || revision != nextRevision || policy != nextPolicy || boundaries != staged {latch=nil;cancelDwell()}
            policy=nextPolicy
            revision=nextRevision;activation=nextActivation;boundaries=staged;staged.removeAll();refresh(screens)
        default:break
        }
    }
    // All records came from Rust's validated Routes. Native geometry still
    // has to match before an edge may suppress a local physical event.
    func clamp(_ point:CGPoint,_ dx:Double,_ dy:Double,_ allow:Bool,now:TimeInterval=ProcessInfo.processInfo.systemUptime)->CGPoint? {
        guard enabled else {cancelDwell();return nil}
        var match:DesktopScreen?
        for screen in screens where screen.rect.contains(point) {
            if case .some=match {cancelDwell();return nil}
            match=screen
        }
        guard let screen=match else {cancelDwell();return nil}
        let rect=screen.rect
        if let old=latch {
            let inset=old.1=="left" ? point.x-rect.minX : old.1=="right" ? rect.maxX-1-point.x : old.1=="top" ? point.y-rect.minY : rect.maxY-1-point.y
            let threshold=min(16,(old.1=="left" || old.1=="right" ? rect.width : rect.height)/3)
            if old.0 != rect || inset>=threshold {latch=nil}
        }
        for boundary in indexed[screen.id] ?? [] {
            if let position=boundary.position(point,dx,dy) {
                let clamped=CGPoint(x:min(max(point.x+dx,rect.minX),rect.maxX-1),y:min(max(point.y+dy,rect.minY),rect.maxY-1))
                guard boundary.id != 0,allow,!policy.paused,!policy.protects(boundary,position),!latched,now>=cooldown else {cancelDwell();return clamped}
                if policy.dwell_ms>0 {
                    if dwell?.boundary != boundary || now-(dwell?.last ?? now)>0.1 {dwell=RouteDwell(boundary:boundary,since:now,last:now)}
                    dwell?.last=now
                    if now-(dwell?.since ?? now)<Double(policy.dwell_ms)/1000 {return clamped}
                }
                cancelDwell();latch=(rect,boundary.side)
                emit(["kind":"boundary","crossing":["revision":NSNumber(value:revision),"activation":NSNumber(value:activation),"boundary":NSNumber(value:boundary.id),"position":position]])
                return clamped
            }
        }
        cancelDwell()
        return nil
    }
}
func layoutSelfTest() throws {
    let rect=RouteRect(x:0,y:0,width:100,height:100)
    let right=RouteBoundary(id:9,screen:rect,geometry:UInt64.max,side:"right",start:0,end:UInt16.max)
    guard right.position(CGPoint(x:98,y:50),2,0) != nil,
          right.position(CGPoint(x:50,y:50),2,0) == nil,
          right.position(CGPoint(x:99,y:50),-2,0) == nil else {throw fail("Native layout edge projection failed")}
    let routes=CaptureRoutes();routes.enabled=true
    routes.refresh([DesktopScreen(id:"1",rect:rect.rect,epoch:UInt64.max)])
    routes.command(["kind":"routes-start"])
    let object:[String:Any]=["id":NSNumber(value:right.id),"screen":["x":0,"y":0,"width":100,"height":100],"geometry":NSNumber(value:UInt64.max),"side":"right","start":0,"end":65535]
    routes.command(["kind":"routes-add","boundary":object])
    routes.command(["kind":"routes-commit","revision":NSNumber(value:UInt64.max),"activation":NSNumber(value:UInt64.max)])
    guard routes.revision==UInt64.max,routes.activation==UInt64.max,routes.indexed["1"]?.count==1,
          routes.clamp(CGPoint(x:98,y:50),4,0,false)==CGPoint(x:99,y:50) else {throw fail("Native layout policy did not preserve its generation or blocked edge")}
    routes.refresh([DesktopScreen(id:"1",rect:rect.rect,epoch:1)])
    guard routes.clamp(CGPoint(x:98,y:50),4,0,false)==nil else {throw fail("Stale native display geometry was accepted")}
    routes.refresh([DesktopScreen(id:"1",rect:rect.rect,epoch:UInt64.max),DesktopScreen(id:"2",rect:rect.rect,epoch:UInt64.max)])
    guard routes.indexed.isEmpty else {throw fail("Ambiguous native geometry was accepted")}
    func install(_ target:CaptureRoutes,_ delay:UInt16=150,_ protect:Bool=true,_ paused:Bool=false,_ revision:UInt64=1) {
        target.command(["kind":"routes-start"])
        target.command(["kind":"routes-add","boundary":object])
        target.command(["kind":"routes-commit","revision":NSNumber(value:revision),"activation":NSNumber(value:1),"policy":["dwell_ms":delay,"protect_corners":protect,"paused":paused]])
    }
    let pressure=CaptureRoutes();pressure.enabled=true
    pressure.refresh([DesktopScreen(id:"1",rect:rect.rect,epoch:UInt64.max)])
    var hits=0
    pressure.emit={_ in hits+=1}
    install(pressure)
    _=pressure.clamp(CGPoint(x:98,y:50),4,0,true,now:0)
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:0.08)
    guard hits==0 else {throw fail("A brief native edge touch bypassed dwell")}
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:0.151)
    guard hits==1 else {throw fail("Continued native edge pressure did not cross")}
    install(pressure,150,true,false,2);hits=0
    _=pressure.clamp(CGPoint(x:98,y:50),4,0,true,now:1)
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:1.08)
    _=pressure.clamp(CGPoint(x:99,y:50),-4,0,true,now:1.1)
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:1.151)
    guard hits==0 else {throw fail("Native pullback did not cancel dwell")}
    pressure.cancelDwell() // A complete key press between pointer events.
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:1.231)
    guard hits==0 else {throw fail("Native held input reused old dwell")}
    install(pressure,150,true,true,3)
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:2)
    install(pressure,150,true,false,4)
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:2.08)
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:4)
    guard hits==0 else {throw fail("Native unlock or parked cursor reused old dwell")}
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:4.08)
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:4.151)
    guard hits==1,pressure.revision==4 else {throw fail("Native policy generation did not resume with fresh dwell")}
    install(pressure,0,true,false,5);hits=0
    _=pressure.clamp(CGPoint(x:99,y:1),4,0,true,now:5)
    guard hits==0 else {throw fail("A native protected corner crossed")}
    _=pressure.clamp(CGPoint(x:99,y:50),4,0,true,now:5.01)
    guard hits==1 else {throw fail("Native corner protection blocked the edge center")}
    for scale in [UInt32(1),UInt32(2)] {
        for side in ["left","right","top","bottom"] {
            let scaled=RouteBoundary(id:1,screen:RouteRect(x:-1000,y:20,width:1000*scale,height:800*scale),geometry:1,side:side,start:0,end:UInt16.max)
            guard RoutePolicy().protects(scaled,655),RoutePolicy().protects(scaled,64880),!RoutePolicy().protects(scaled,8192) else {throw fail("Native corner protection changed with coordinate scaling")}
        }
    }
    print("Native layout policy: projection, generations, blocked edges, stale geometry, dwell, pullback, corners and pause passed")
}
func edgePoint(_ display:CGDirectDisplayID,_ side:String,_ position:Int)->CGPoint {
    let bounds=CGDisplayBounds(display), fraction=Double(position)/65535
    let inset=min(16,min(bounds.width,bounds.height)/3)
    let x=bounds.minX+fraction*max(1,bounds.width-1), y=bounds.minY+fraction*max(1,bounds.height-1)
    switch side {
    case "left":return CGPoint(x:bounds.minX+inset,y:y)
    case "right":return CGPoint(x:bounds.maxX-1-inset,y:y)
    case "top":return CGPoint(x:x,y:bounds.minY+inset)
    default:return CGPoint(x:x,y:bounds.maxY-1-inset)
    }
}

final class PCMWriter {
    private let lock = NSCondition()
    private var queue = [Data]()
    init() {
        DispatchQueue(label: "Desk Switch PCM writer", qos: .userInteractive).async { [self] in
            while true {
                lock.lock(); while queue.isEmpty { lock.wait() }; let data = queue.removeFirst(); lock.unlock()
                FileHandle.standardOutput.write(data)
            }
        }
    }
    func append(_ data: Data) {
        lock.lock(); if queue.count >= 24 { queue.removeFirst() }; queue.append(data); lock.signal(); lock.unlock()
    }
}
final class PCMConverter {
    let writer = PCMWriter()
    let target = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 48000, channels: 2, interleaved: true)!
    var converter: AVAudioConverter?
    var previous: AVAudioFormat?
    func consume(_ input: AVAudioPCMBuffer) {
        if previous != input.format { previous = input.format; converter = AVAudioConverter(from: input.format, to: target) }
        guard let converter = converter, let output = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: AVAudioFrameCount(Double(input.frameLength) * 48000 / input.format.sampleRate + 64)) else { return }
        var provided = false, error: NSError?
        let status = converter.convert(to: output, error: &error) { _, result in
            if provided { result.pointee = .noDataNow; return nil }
            provided = true; result.pointee = .haveData; return input
        }
        guard status != .error, output.frameLength > 0, let bytes = output.audioBufferList.pointee.mBuffers.mData else { return }
        writer.append(Data(bytes: bytes, count: Int(output.frameLength) * 4))
    }
}
@available(macOS 13.0, *)
final class SystemCapture: NSObject, SCStreamOutput, SCStreamDelegate {
    var stream: SCStream?
    let converter = PCMConverter()
    var included = Set<pid_t>()
    func applications(_ content:SCShareableContent)->[SCRunningApplication] {
        content.applications.filter { application in
            let name=application.applicationName.lowercased()
            let identifier=application.bundleIdentifier.lowercased()
            return !["com.deskswitch.desktop","com.deskswitch.nativehelper"].contains(identifier) && !["desk switch","desk-switch","desk-switch-native"].contains(name)
        }
    }
    func start() async throws {
        guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else { throw fail("Allow Desk Switch in System Settings → Privacy & Security → Screen Recording, then restart sharing") }
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let display = content.displays.first else { throw fail("No macOS display is available for system audio capture") }
        // An allow-list keeps later-started Desk Switch playback workers out
        // of system capture too, including the virtual microphone feed.
        let allowed=applications(content);included=Set(allowed.map(\.processID))
        let filter = SCContentFilter(display: display, including: allowed, exceptingWindows: [])
        let config = SCStreamConfiguration()
        config.width = 2; config.height = 2; config.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        config.showsCursor = false; config.queueDepth = 3
        config.capturesAudio = true; config.excludesCurrentProcessAudio = true; config.sampleRate = 48000; config.channelCount = 2
        let stream = SCStream(filter: filter, configuration: config, delegate: self)
        try stream.addStreamOutput(self, type: .audio, sampleHandlerQueue: DispatchQueue(label: "Desk Switch audio capture", qos: .userInteractive))
        self.stream = stream
        try await stream.startCapture()
        ready("macOS system audio")
        Task { [weak self] in
            while let self=self {
                try? await Task.sleep(nanoseconds:2_000_000_000)
                do {
                    let content=try await SCShareableContent.excludingDesktopWindows(false,onScreenWindowsOnly:false)
                    let allowed=self.applications(content), ids=Set(allowed.map(\.processID))
                    if ids != self.included, let display=content.displays.first {
                        try await stream.updateContentFilter(SCContentFilter(display:display,including:allowed,exceptingWindows:[]))
                        self.included=ids
                    }
                } catch {FileHandle.standardError.write(Data("Updating the macOS application audio filter: \(error.localizedDescription)\n".utf8))}
            }
        }
    }
    func stream(_ stream: SCStream, didStopWithError error: Error) {
        FileHandle.standardError.write(Data("macOS system audio stopped: \(error.localizedDescription)\n".utf8)); exit(1)
    }
    func stream(_ stream: SCStream, didOutputSampleBuffer sampleBuffer: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sampleBuffer.isValid, let description = sampleBuffer.formatDescription else { return }
        let format = AVAudioFormat(cmAudioFormatDescription: description)
        var needed = 0
        CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sampleBuffer, bufferListSizeNeededOut: &needed, bufferListOut: nil, bufferListSize: 0, blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: 0, blockBufferOut: nil)
        guard needed > 0 && needed <= 4096 else { return }
        let memory = UnsafeMutableRawPointer.allocate(byteCount: needed, alignment: 16)
        defer { memory.deallocate() }
        let list = memory.assumingMemoryBound(to: AudioBufferList.self)
        var retained: CMBlockBuffer?
        guard CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(sampleBuffer, bufferListSizeNeededOut: nil, bufferListOut: list, bufferListSize: needed, blockBufferAllocator: nil, blockBufferMemoryAllocator: nil, flags: UInt32(kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment), blockBufferOut: &retained) == noErr,
              let buffer = AVAudioPCMBuffer(pcmFormat: format, bufferListNoCopy: list, deallocator: nil) else { return }
        buffer.frameLength = AVAudioFrameCount(CMSampleBufferGetNumSamples(sampleBuffer))
        converter.consume(buffer)
        withExtendedLifetime(retained) {}
    }
}

func selectAudioUnit(_ unit: AudioUnit?, _ id: AudioDeviceID) throws {
    guard let unit = unit else { throw fail("CoreAudio unit is unavailable") }
    var id = id
    try checked(AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0, &id, UInt32(MemoryLayout<AudioDeviceID>.size)), "Selecting the audio device")
}
final class MicrophoneCapture {
    let engine = AVAudioEngine(), converter = PCMConverter()
    func start(_ uid: String?) async throws {
        guard await AVCaptureDevice.requestAccess(for: .audio) else { throw fail("Allow Desk Switch microphone access in System Settings → Privacy & Security → Microphone") }
        let id = try device(uid, input: true), input = engine.inputNode
        guard !cable(try stringProperty(id,kAudioObjectPropertyName)) else {throw fail("A virtual microphone feed cannot be shared as a physical microphone")}
        try selectAudioUnit(input.audioUnit, id)
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0 && format.channelCount > 0 else { throw fail("The selected microphone is not ready") }
        input.installTap(onBus: 0, bufferSize: 480, format: format) { [self] buffer, _ in converter.consume(buffer) }
        try engine.start()
        ready(try stringProperty(id, kAudioObjectPropertyName))
    }
}
final class AudioPlayback {
    let engine = AVAudioEngine(), player = AVAudioPlayerNode()
    let capacity = DispatchSemaphore(value: 8)
    func run(_ uid: String?) throws {
        let id = try device(uid, input: false)
        try selectAudioUnit(engine.outputNode.audioUnit, id)
        let format = AVAudioFormat(standardFormatWithSampleRate: 48000, channels: 2)!
        engine.attach(player); engine.connect(player, to: engine.mainMixerNode, format: format)
        try engine.start(); player.play()
        ready(try stringProperty(id, kAudioObjectPropertyName))
        while true {
            var block = Data()
            while block.count < 1920 {
                guard let next = try FileHandle.standardInput.read(upToCount: 1920 - block.count), !next.isEmpty else { return }
                block.append(next)
            }
            capacity.wait()
            guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 480), let channels = buffer.floatChannelData else { throw fail("Cannot allocate the playback buffer") }
            buffer.frameLength = 480
            block.withUnsafeBytes { (bytes: UnsafeRawBufferPointer) in
                for frame in 0..<480 { for channel in 0..<2 {
                    let offset = (frame * 2 + channel) * 2
                    let value = Int16(bitPattern: UInt16(bytes[offset]) | UInt16(bytes[offset + 1]) << 8)
                    channels[channel][frame] = Float(value) / 32768
                }}
            }
            player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { [capacity] _ in capacity.signal() }
        }
    }
}

let injectedMarker: Int64 = 0x44535749544348
final class InputCapture {
    var tap: CFMachPort?
    var mode = "local", deadline = CFAbsoluteTimeGetCurrent(), hidden = false
    var keyboard = true, mouse = true
    var edgeDisplay: CGDirectDisplayID?, edgeSide="right", edgeAutomatic=false
    var keys = [Int64: Bool](), buttons = [Int64: Bool]()
    let routes=CaptureRoutes()
    var localPosition=CGPoint.zero
    func setMode(_ value: String) {
        if mode != value {routes.cancelDwell()}
        if mode != value && value=="local" {localPosition=CGEvent(source:nil)?.location ?? localPosition}
        mode = value; deadline = CFAbsoluteTimeGetCurrent() + 0.25
        if value == "remote" && mouse && !hidden { CGDisplayHideCursor(CGMainDisplayID()); hidden = true }
        if value == "local" && hidden { CGDisplayShowCursor(CGMainDisplayID()); hidden = false }
    }
    func event(_ type: CGEventType, _ event: CGEvent) -> Unmanaged<CGEvent>? {
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            setMode("local"); if let tap = tap { CGEvent.tapEnable(tap: tap, enable: true) }; json(["kind": "emergency"]); return Unmanaged.passUnretained(event)
        }
        if event.getIntegerValueField(.eventSourceUserData) == injectedMarker { return Unmanaged.passUnretained(event) }
        if CFAbsoluteTimeGetCurrent() >= deadline { setMode("local") }
        let remote = mode != "local"
        if type.rawValue==UInt32(NSEvent.EventType.systemDefined.rawValue),let native=NSEvent(cgEvent:event),native.subtype.rawValue==8 {
            // IOHIDSystem's documented NX_KEYTYPE sound-up/down/mute events.
            let mapping:[Int:Int64]=[0:72,1:73,7:74], code=(native.data1>>16)&0xffff, state=(native.data1>>8)&0xff
            if let code=mapping[code],state==0x0a || state==0x0b {
                routes.cancelDwell()
                let down=state==0x0a
                if keys[code]==nil && down {keys[code]=remote && keyboard}
                let owner=keys[code] ?? false;json(["kind":"key","code":code,"down":down,"remote":owner])
                if !down {keys.removeValue(forKey:code)}
                return owner ? nil : Unmanaged.passUnretained(event)
            }
        }
        switch type {
        case .keyDown, .keyUp, .flagsChanged:
            routes.cancelDwell()
            let code = event.getIntegerValueField(.keyboardEventKeycode)
            if code == 103 { // F11 always returns local input, even during a failed handoff.
                if type == .keyDown { setMode("local"); json(["kind": "emergency"]) }; return nil
            }
            if code == 111 { if type == .keyDown && event.getIntegerValueField(.keyboardEventAutorepeat) == 0 { setMode("hold"); json(["kind": "toggle"]) }; return nil }
            var down = type == .keyDown
            if type == .flagsChanged {
                if code == 57 {
                    json(["kind": "key", "code": code, "down": true, "remote": remote && keyboard])
                    json(["kind": "key", "code": code, "down": false, "remote": remote && keyboard])
                    return remote && keyboard ? nil : Unmanaged.passUnretained(event)
                }
                down = CGEventSource.keyState(.hidSystemState, key: CGKeyCode(code))
            }
            if keys[code] == nil && down { keys[code] = remote && keyboard }
            let owner = keys[code] ?? false
            json(["kind": "key", "code": code, "down": down, "remote": owner])
            if !down { keys.removeValue(forKey: code) }
            return owner ? nil : Unmanaged.passUnretained(event)
        case .leftMouseDown, .rightMouseDown, .otherMouseDown, .leftMouseUp, .rightMouseUp, .otherMouseUp:
            routes.cancelDwell()
            let button = event.getIntegerValueField(.mouseEventButtonNumber)
            let down = type == .leftMouseDown || type == .rightMouseDown || type == .otherMouseDown
            if buttons[button] == nil && down { buttons[button] = remote && mouse }
            let owner = buttons[button] ?? false
            json(["kind": "button", "code": button, "down": down, "remote": owner])
            if !down { buttons.removeValue(forKey: button) }
            return owner ? nil : Unmanaged.passUnretained(event)
        case .mouseMoved, .leftMouseDragged, .rightMouseDragged, .otherMouseDragged:
            if remote && mouse { json(["kind": "move", "dx": event.getIntegerValueField(.mouseEventDeltaX), "dy": event.getIntegerValueField(.mouseEventDeltaY), "remote": true]); return nil }
            if mouse && routes.enabled {
                // Location deltas retain the user's macOS acceleration. At a
                // physical screen limit the raw delta detects continued push.
                let p=event.location,previous=localPosition
                let rawX=Double(event.getIntegerValueField(.mouseEventDeltaX)),rawY=Double(event.getIntegerValueField(.mouseEventDeltaY))
                let dx=p.x==previous.x ? rawX : p.x-previous.x,dy=p.y==previous.y ? rawY : p.y-previous.y
                if let clamped=routes.clamp(previous,dx,dy,keys.isEmpty && buttons.isEmpty) {event.location=clamped;localPosition=clamped} else {localPosition=p}
                return Unmanaged.passUnretained(event)
            }
            localPosition=event.location
            if mouse && keys.isEmpty && buttons.isEmpty, let display=edgeAutomatic ? CGMainDisplayID() : edgeDisplay, CGDisplayIsActive(display) != 0 {
                let bounds=CGDisplayBounds(display), p=event.location
                let dx=event.getIntegerValueField(.mouseEventDeltaX), dy=event.getIntegerValueField(.mouseEventDeltaY)
                let crossing=bounds.contains(p) && ((edgeSide=="left" && dx<0 && p.x<=bounds.minX+1) || (edgeSide=="right" && dx>0 && p.x>=bounds.maxX-2) || (edgeSide=="top" && dy<0 && p.y<=bounds.minY+1) || (edgeSide=="bottom" && dy>0 && p.y>=bounds.maxY-2))
                if crossing {
                    let fraction = edgeSide=="left" || edgeSide=="right" ? (p.y-bounds.minY)/max(1,bounds.height-1) : (p.x-bounds.minX)/max(1,bounds.width-1)
                    setMode("hold");json(["kind":"edge","position":Int((min(1,max(0,fraction))*65535).rounded())]);return nil
                }
            }
        case .scrollWheel:
            if remote && mouse { json(["kind": "scroll", "dx": max(-4096,min(4096,event.getIntegerValueField(.scrollWheelEventDeltaAxis2) * 120)), "dy": max(-4096,min(4096,-event.getIntegerValueField(.scrollWheelEventDeltaAxis1) * 120)), "remote": true]); return nil }
        default: break
        }
        return Unmanaged.passUnretained(event)
    }
    func command(_ value: [String: Any]) {
        switch value["kind"] as? String {
        case "mode": setMode(value["value"] as? String ?? "local")
        case "toggle": setMode("hold"); json(["kind": "toggle"])
        case "center": let bounds = CGDisplayBounds(CGMainDisplayID()); CGWarpMouseCursorPosition(CGPoint(x: bounds.midX, y: bounds.midY))
        case "enter":
            let display=(value["screen"] as? String).flatMap(UInt32.init) ?? CGMainDisplayID()
            CGWarpMouseCursorPosition(edgePoint(display,value["side"] as? String ?? "left",value["position"] as? Int ?? 32768))
        case "reset-entry":localPosition=CGEvent(source:nil)?.location ?? localPosition;routes.resetEntry()
        case "routes-start","routes-add","routes-commit":routes.command(value)
        case "stop": setMode("local"); exit(0)
        default: break
        }
    }
    func run(_ sharing: [String]) throws {
        keyboard = sharing.first != "false"; mouse = sharing.dropFirst().first != "false"
        routes.enabled=sharing.contains("--group")
        if sharing.count>=4 && mouse && !routes.enabled {edgeAutomatic=sharing[2]=="auto";edgeDisplay=edgeAutomatic ? CGMainDisplayID() : UInt32(sharing[2]);edgeSide=sharing[3];guard let display=edgeDisplay, CGDisplayIsActive(display) != 0 else {throw fail("The selected macOS edge screen is unavailable")}}
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        guard AXIsProcessTrustedWithOptions(options) else { throw fail("Allow Desk Switch in System Settings → Privacy & Security → Accessibility, then retry input sharing") }
        guard CGPreflightListenEventAccess() || CGRequestListenEventAccess() else { throw fail("Allow Desk Switch in System Settings → Privacy & Security → Input Monitoring, then retry input sharing") }
        let types: [CGEventType] = [.keyDown,.keyUp,.flagsChanged,.leftMouseDown,.leftMouseUp,.rightMouseDown,.rightMouseUp,.otherMouseDown,.otherMouseUp,.mouseMoved,.leftMouseDragged,.rightMouseDragged,.otherMouseDragged,.scrollWheel]
        let mask = types.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << $1.rawValue) } | (CGEventMask(1)<<NSEvent.EventType.systemDefined.rawValue)
        guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap, options: .defaultTap, eventsOfInterest: mask, callback: { _,type,event,context in
            guard let context = context else { return Unmanaged.passUnretained(event) }
            return Unmanaged<InputCapture>.fromOpaque(context).takeUnretainedValue().event(type,event)
        }, userInfo: Unmanaged.passUnretained(self).toOpaque()) else { throw fail("macOS refused the keyboard/mouse event tap; check Accessibility and Input Monitoring") }
        self.tap = tap
        routes.refresh(try desktopScreens());localPosition=CGEvent(source:nil)?.location ?? .zero
        CGDisplayRegisterReconfigurationCallback({_,flags,context in
            if !flags.contains(.beginConfigurationFlag),let context=context {
                let capture=Unmanaged<InputCapture>.fromOpaque(context).takeUnretainedValue()
                DispatchQueue.main.async {
                    if let screens=try? desktopScreens() {capture.routes.refresh(screens)} else {capture.routes.refresh([]);capture.setMode("local");json(["kind":"emergency"])}
                    capture.localPosition=CGEvent(source:nil)?.location ?? .zero
                }
            }
        },Unmanaged.passUnretained(self).toOpaque())
        CFRunLoopAddSource(CFRunLoopGetMain(), CFMachPortCreateRunLoopSource(nil, tap, 0), .commonModes)
        CGEvent.tapEnable(tap: tap, enable: true)
        let timer = CFRunLoopTimerCreateWithHandler(nil, CFAbsoluteTimeGetCurrent(), 0.05, 0, 0) { [self] _ in if CFAbsoluteTimeGetCurrent() >= deadline || getppid() == 1 { setMode("local") }; if getppid() == 1 { exit(0) } }
        CFRunLoopAddTimer(CFRunLoopGetMain(), timer, .commonModes)
        DispatchQueue.global(qos: .userInteractive).async { [self] in
            while let line = readLine() {
                if line.utf8.count<=512,let data = line.data(using: .utf8), let value = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] { DispatchQueue.main.async { self.command(value) } }
            }
            DispatchQueue.main.async { self.setMode("local"); exit(0) }
        }
        ready("macOS keyboard and mouse")
        CFRunLoopRun()
        setMode("local")
    }
}

final class InputPlayback {
    var keys = Set<Int>(), buttons = Set<Int>()
    var position = CGPoint.zero
    var wheelX = 0, wheelY = 0
    var active=false, deadline=CFAbsoluteTimeGetCurrent()
    var tap:CFMachPort?
    let source = CGEventSource(stateID: .privateState)
    func post(_ event: CGEvent?) {
        guard let event = event else { return }
        event.setIntegerValueField(.eventSourceUserData, value: injectedMarker)
        event.post(tap: .cghidEventTap)
    }
    func release() {
        for key in keys { keyEvent(key,false) }; keys.removeAll()
        for button in buttons { mouse(button, false) }; buttons.removeAll()
        active=false;wheelX=0;wheelY=0
    }
    func mouse(_ button: Int, _ down: Bool) {
        let type: CGEventType = button == 0 ? (down ? .leftMouseDown : .leftMouseUp) : button == 1 ? (down ? .rightMouseDown : .rightMouseUp) : (down ? .otherMouseDown : .otherMouseUp)
        post(CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: position, mouseButton: CGMouseButton(rawValue: UInt32(button)) ?? .left))
    }
    func keyEvent(_ code:Int,_ down:Bool) {
        if let media=[72:0,73:1,74:7][code] {
            let state=down ? 0x0a : 0x0b
            let event=NSEvent.otherEvent(with:.systemDefined,location:.zero,modifierFlags:NSEvent.ModifierFlags(rawValue:UInt(state<<8)),timestamp:0,windowNumber:0,context:nil,subtype:8,data1:(media<<16)|(state<<8),data2:-1)
            post(event?.cgEvent)
        } else {post(CGEvent(keyboardEventSource:source,virtualKey:CGKeyCode(code),keyDown:down))}
    }
    func command(_ value: [String: Any]) {
        switch value["kind"] as? String {
        case "activate":
            if let x=value["x"] as? Double, let y=value["y"] as? Double {position=CGPoint(x:x,y:y);if value["warp"] as? Bool != false {CGWarpMouseCursorPosition(position)}} else {position=CGEvent(source:nil)?.location ?? .zero}
            active=true;deadline=CFAbsoluteTimeGetCurrent()+0.25
        case "lease":deadline=CFAbsoluteTimeGetCurrent()+0.25
        case "release": release()
        case "key":
            guard active, let code = value["code"] as? Int, (0..<128).contains(code), let down = value["down"] as? Bool else { return }
            if down { keys.insert(code) } else { keys.remove(code) }; keyEvent(code,down)
        case "button":
            guard active, let code = value["code"] as? Int, (0...4).contains(code), let down = value["down"] as? Bool else { return }
            if down { buttons.insert(code) } else { buttons.remove(code) }; mouse(code,down)
        case "move":
            guard active,let x=value["x"] as? Double,let y=value["y"] as? Double,x.isFinite,y.isFinite else {return}
            position=CGPoint(x:x,y:y)
            let type: CGEventType = buttons.contains(0) ? .leftMouseDragged : buttons.contains(1) ? .rightMouseDragged : buttons.isEmpty ? .mouseMoved : .otherMouseDragged
            post(CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: position, mouseButton: .left))
        case "scroll":
            guard active else {return}
            let dx = value["dx"] as? Int ?? 0, dy = value["dy"] as? Int ?? 0
            if abs(dx) <= 4096 && abs(dy) <= 4096 {
                wheelX += dx; wheelY -= dy
                let x = wheelX / 120, y = wheelY / 120; wheelX %= 120; wheelY %= 120
                if x != 0 || y != 0 { post(CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 2, wheel1: Int32(y), wheel2: Int32(x), wheel3: 0)) }
            }
        case "stop": release(); exit(0)
        default: break
        }
    }
    func run() throws {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        guard AXIsProcessTrustedWithOptions(options) else { throw fail("Allow Desk Switch in System Settings → Privacy & Security → Accessibility, then retry input sharing") }
        position = CGEvent(source: nil)?.location ?? .zero
        tap=CGEvent.tapCreate(tap:.cgSessionEventTap,place:.headInsertEventTap,options:.listenOnly,eventsOfInterest:CGEventMask(1)<<CGEventType.keyDown.rawValue,callback:{_,type,event,context in
            if type == .keyDown && event.getIntegerValueField(.keyboardEventKeycode)==103 && event.getIntegerValueField(.eventSourceUserData) != injectedMarker, let context=context {
                let receiver=Unmanaged<InputPlayback>.fromOpaque(context).takeUnretainedValue();receiver.release();json(["kind":"emergency"])
            }
            return Unmanaged.passUnretained(event)
        },userInfo:Unmanaged.passUnretained(self).toOpaque())
        if let tap=tap {CFRunLoopAddSource(CFRunLoopGetMain(),CFMachPortCreateRunLoopSource(nil,tap,0),.commonModes);CGEvent.tapEnable(tap:tap,enable:true)}
        let context=Unmanaged.passUnretained(self).toOpaque()
        CGDisplayRegisterReconfigurationCallback({_,flags,context in
            if !flags.contains(.beginConfigurationFlag),let context=context {
                let receiver=Unmanaged<InputPlayback>.fromOpaque(context).takeUnretainedValue()
                DispatchQueue.main.async{
                    if let screens=try? screenGeometryValue(){json(["kind":"geometry","screens":screens])}else{receiver.release();json(["kind":"emergency"])}
                }
            }
        },context)
        let timer=CFRunLoopTimerCreateWithHandler(nil,CFAbsoluteTimeGetCurrent(),0.05,0,0){[self] _ in
            if active && CFAbsoluteTimeGetCurrent()>=deadline {release();json(["kind":"emergency"])}
            if getppid()==1 {release();exit(0)}
        }
        CFRunLoopAddTimer(CFRunLoopGetMain(),timer,.commonModes)
        ready("macOS input receiver")
        json(["kind":"geometry","screens":try screenGeometryValue()])
        if tap==nil {FileHandle.standardError.write(Data("The local F11 listener is unavailable; F11 on the input-owning PC still restores input. Enable Input Monitoring for a local emergency key.\n".utf8))}
        DispatchQueue.global(qos:.userInteractive).async {[self] in
            while let line=readLine() {
                guard line.utf8.count<=1024,let data=line.data(using:.utf8),let value=(try? JSONSerialization.jsonObject(with:data)) as? [String:Any] else {continue}
                DispatchQueue.main.async{self.command(value)}
            }
            DispatchQueue.main.async{self.release();exit(0)}
        }
        CFRunLoopRun()
        release()
    }
}

@main struct NativeHelper {
    static func main() async {
        // Broken pipes stop this worker; no helper is kept after its owner exits.
        signal(SIGPIPE, SIG_DFL)
        let args = Array(CommandLine.arguments.dropFirst())
        do {
            switch args.first {
            case "--version": print("Desk Switch native helper 1")
            case "layout-self-test":try layoutSelfTest()
            case "bundle-info":
                guard args.count==2 else {throw fail("A bundle Info.plist path is required")}
                let data=try Data(contentsOf:URL(fileURLWithPath:args[1]),options:.mappedIfSafe)
                guard data.count<=65536,let info=try PropertyListSerialization.propertyList(from:data,options:[],format:nil) as? [String:Any] else {throw fail("The application Info.plist is invalid")}
                let keys=["CFBundleIdentifier","CFBundleExecutable","CFBundlePackageType","CFBundleShortVersionString","CFBundleVersion","LSMinimumSystemVersion"]
                json(info.filter{keys.contains($0.key)})
            case "devices": try devices()
            case "input-devices": try inputDevices()
            case "displays": try displays()
            case "screen-geometry":json(try screenGeometryValue())
            case "cursor":let point=CGEvent(source:nil)?.location ?? .zero;json(["x":point.x,"y":point.y])
            case "volume": try volume(args.count > 1 && args[1] != "-" ? args[1] : nil, set: Array(args.dropFirst(2)))
            case "volume-server":
                let uid=args.count>1 && args[1] != "-" ? args[1] : nil
                _=try device(uid,input:false);ready("macOS output volume")
                while let line=readLine() {
                    guard line.utf8.count<=1024,let data=line.data(using:.utf8),let value=(try? JSONSerialization.jsonObject(with:data)) as? [String:String] else {throw fail("Invalid system-volume command")}
                    do {try volume(uid,set:[value["volume"] ?? "-",value["muted"] ?? "-"])}catch{json(["error":String(describing:error)])}
                }
            case "input-capture": try InputCapture().run(Array(args.dropFirst()))
            case "input-playback": try InputPlayback().run()
            case "playback": try AudioPlayback().run(args.count > 1 ? args[1] : nil)
            case "capture-microphone":
                let capture = MicrophoneCapture(); try await capture.start(args.count > 1 ? args[1] : nil)
                while true { try await Task.sleep(nanoseconds: 1_000_000_000); if getppid() == 1 { return }; withExtendedLifetime(capture) {} }
            case "capture-system":
                if #available(macOS 13.0, *) {
                    let capture = SystemCapture(); try await capture.start()
                    while true { try await Task.sleep(nanoseconds: 1_000_000_000); if getppid() == 1 { return }; withExtendedLifetime(capture) {} }
                } else { throw fail("System audio capture requires macOS 13 or newer") }
            default: throw fail("Unknown Desk Switch native helper command")
            }
        } catch {
            FileHandle.standardError.write(Data("Desk Switch macOS: \(error)\n".utf8)); exit(1)
        }
    }
}
