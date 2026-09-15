import AppKit
import ApplicationServices
import CoreGraphics
import Darwin
import Foundation
import ImageIO
import ScreenCaptureKit

struct WindowRecord: Codable {
    let window_id: UInt32
    let pid: Int32
    let application_name: String
    let bundle_identifier: String
    let title: String?
}

enum BridgeError: Error, CustomStringConvertible {
    case usage(String)
    case targetNotFound(UInt32)
    case targetIdentityChanged
    case accessibilityPermissionMissing
    case screenshotEncodingFailed

    var description: String {
        switch self {
        case .usage(let message): return message
        case .targetNotFound(let id): return "window \(id) was not found on screen"
        case .targetIdentityChanged: return "target window identity changed; refusing operation"
        case .accessibilityPermissionMissing:
            return "macOS Accessibility permission is required for keyboard input; grant it to the terminal running AgentPlay and retry"
        case .screenshotEncodingFailed: return "failed to encode screenshot as PNG"
        }
    }
}

final class WindowSession {
    let windowID: UInt32
    let pid: Int32
    let bundleID: String
    let filter: SCContentFilter
    let configuration: SCStreamConfiguration

    init(window: SCWindow, pid: Int32, bundleID: String) {
        windowID = window.windowID
        self.pid = pid
        self.bundleID = bundleID
        filter = SCContentFilter(desktopIndependentWindow: window)
        configuration = SCStreamConfiguration()
        configuration.width = max(1, Int(filter.contentRect.width * CGFloat(filter.pointPixelScale)))
        configuration.height = max(1, Int(filter.contentRect.height * CGFloat(filter.pointPixelScale)))
        configuration.showsCursor = false
    }

    func validateLiveTarget() throws {
        guard let records = CGWindowListCopyWindowInfo(
            .optionIncludingWindow,
            CGWindowID(windowID)
        ) as? [[String: Any]],
              let record = records.first else {
            throw BridgeError.targetNotFound(windowID)
        }
        guard let owner = record[kCGWindowOwnerPID as String] as? NSNumber,
              owner.int32Value == pid,
              let app = NSRunningApplication(processIdentifier: pid),
              app.bundleIdentifier == bundleID else {
            throw BridgeError.targetIdentityChanged
        }
    }

    func capture() async throws -> Data {
        try validateLiveTarget()
        let image = try await SCScreenshotManager.captureImage(
            contentFilter: filter,
            configuration: configuration
        )
        return try AgentPlayMacOSBridge.encodePNG(image)
    }

    func press(keyCode: CGKeyCode, holdMillis: UInt64) async throws {
        try validateLiveTarget()
        let accessibilityOptions = [
            kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true
        ] as CFDictionary
        guard AXIsProcessTrustedWithOptions(accessibilityOptions) else {
            throw BridgeError.accessibilityPermissionMissing
        }
        guard let source = CGEventSource(stateID: .hidSystemState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: keyCode, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: keyCode, keyDown: false) else {
            throw BridgeError.usage("failed to create CoreGraphics keyboard event")
        }

        down.postToPid(pid)
        if holdMillis > 0 {
            try await Task.sleep(for: .milliseconds(holdMillis))
        }
        up.postToPid(pid)
    }
}

@main
struct AgentPlayMacOSBridge {
    static func main() async {
        do {
            try await run()
        } catch {
            FileHandle.standardError.write(Data("agentplay macOS bridge: \(error)\n".utf8))
            exit(1)
        }
    }

    static func run() async throws {
        _ = CGMainDisplayID()

        let args = Array(CommandLine.arguments.dropFirst())
        guard let command = args.first else {
            throw BridgeError.usage("expected command: list | capture | press | session")
        }

        switch command {
        case "list":
            guard args.count == 1 else { throw BridgeError.usage("usage: list") }
            try await listWindows()
        case "capture":
            guard args.count == 4,
                  let windowID = UInt32(args[1]),
                  let pid = Int32(args[2]) else {
                throw BridgeError.usage("usage: capture <window-id> <pid> <bundle-id>")
            }
            try await capture(windowID: windowID, pid: pid, bundleID: args[3])
        case "press":
            guard args.count == 6,
                  let windowID = UInt32(args[1]),
                  let pid = Int32(args[2]),
                  let keyCode = UInt16(args[4]),
                  let holdMillis = UInt64(args[5]) else {
                throw BridgeError.usage("usage: press <window-id> <pid> <bundle-id> <key-code> <hold-ms>")
            }
            try await press(
                windowID: windowID,
                pid: pid,
                bundleID: args[3],
                keyCode: CGKeyCode(keyCode),
                holdMillis: holdMillis
            )
        case "session":
            guard args.count == 4,
                  let windowID = UInt32(args[1]),
                  let pid = Int32(args[2]) else {
                throw BridgeError.usage("usage: session <window-id> <pid> <bundle-id>")
            }
            try await runSession(windowID: windowID, pid: pid, bundleID: args[3])
        default:
            throw BridgeError.usage("unknown command \(command)")
        }
    }

    static func shareableContent() async throws -> SCShareableContent {
        try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
    }

    static func windowRecord(_ window: SCWindow) -> WindowRecord? {
        guard let app = window.owningApplication else { return nil }
        return WindowRecord(
            window_id: window.windowID,
            pid: app.processID,
            application_name: app.applicationName,
            bundle_identifier: app.bundleIdentifier,
            title: window.title
        )
    }

    static func validatedWindow(windowID: UInt32, pid: Int32, bundleID: String) async throws -> SCWindow {
        let content = try await shareableContent()
        guard let window = content.windows.first(where: { $0.windowID == windowID }) else {
            throw BridgeError.targetNotFound(windowID)
        }
        guard let app = window.owningApplication,
              app.processID == pid,
              app.bundleIdentifier == bundleID else {
            throw BridgeError.targetIdentityChanged
        }
        return window
    }

    static func listWindows() async throws {
        let content = try await shareableContent()
        let records = content.windows.compactMap(windowRecord)
        let encoded = try JSONEncoder().encode(records)
        FileHandle.standardOutput.write(encoded)
    }

    static func capture(windowID: UInt32, pid: Int32, bundleID: String) async throws {
        let window = try await validatedWindow(windowID: windowID, pid: pid, bundleID: bundleID)
        let session = WindowSession(window: window, pid: pid, bundleID: bundleID)
        FileHandle.standardOutput.write(try await session.capture())
    }

    static func press(
        windowID: UInt32,
        pid: Int32,
        bundleID: String,
        keyCode: CGKeyCode,
        holdMillis: UInt64
    ) async throws {
        let window = try await validatedWindow(windowID: windowID, pid: pid, bundleID: bundleID)
        let session = WindowSession(window: window, pid: pid, bundleID: bundleID)
        try await session.press(keyCode: keyCode, holdMillis: holdMillis)
    }

    static func runSession(windowID: UInt32, pid: Int32, bundleID: String) async throws {
        let window = try await validatedWindow(windowID: windowID, pid: pid, bundleID: bundleID)
        let session = WindowSession(window: window, pid: pid, bundleID: bundleID)
        writeLine("READY")

        while let line = readLine(strippingNewline: true) {
            let fields = line.split(separator: " ")
            guard let command = fields.first else { continue }

            do {
                switch command {
                case "capture":
                    guard fields.count == 1 else {
                        throw BridgeError.usage("session capture takes no arguments")
                    }
                    let data = try await session.capture()
                    writeLine("OK \(data.count)")
                    FileHandle.standardOutput.write(data)
                case "press":
                    guard fields.count == 3,
                          let keyCode = UInt16(fields[1]),
                          let holdMillis = UInt64(fields[2]) else {
                        throw BridgeError.usage("session press requires <key-code> <hold-ms>")
                    }
                    try await session.press(keyCode: CGKeyCode(keyCode), holdMillis: holdMillis)
                    writeLine("OK")
                default:
                    throw BridgeError.usage("unknown session command \(command)")
                }
            } catch {
                writeLine("ERR \(error)")
            }
        }
    }

    static func encodePNG(_ image: CGImage) throws -> Data {
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(
            data,
            "public.png" as CFString,
            1,
            nil
        ) else {
            throw BridgeError.screenshotEncodingFailed
        }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else {
            throw BridgeError.screenshotEncodingFailed
        }
        return data as Data
    }

    static func writeLine(_ value: String) {
        FileHandle.standardOutput.write(Data("\(value)\n".utf8))
    }
}
