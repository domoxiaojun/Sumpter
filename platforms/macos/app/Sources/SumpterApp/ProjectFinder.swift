import AppKit
import Foundation
import SumpterCore

/// Finder lookup for a project shown by runtime analytics.
///
/// Runtime analytics carries only a redacted workspace suffix. This helper never
/// persists or reconstructs the original absolute path; it resolves the suffix
/// under bounded local roots, then uses a read-only Spotlight fallback when
/// needed.
enum ProjectFinder {
    enum Outcome: Sendable {
        case matches([String])
        case unavailable
    }

    /// Locate using the sanitized workspace suffix carried by the analytics
    /// project row. This is the preferred path because it does not depend on
    /// the UI having loaded a matching recent event.
    static func locate(projectName: String, workspacePaths: [String]) async -> Outcome {
        let name = projectName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isSearchableProject(name) else { return .unavailable }
        let hints = workspacePaths
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
        return await locate(projectName: name, hints: hints)
    }

    /// Compatibility fallback for older daemons whose analytics rows do not
    /// yet include workspacePaths.
    static func locate(projectName: String, events: [RuntimeEvent]) async -> Outcome {
        let name = projectName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isSearchableProject(name) else { return .unavailable }
        let hints = workspacePaths(projectName: name, events: events)
        return await locate(projectName: name, hints: hints)
    }

    private static func locate(projectName: String, hints: [String]) async -> Outcome {
        return await Task.detached(priority: .userInitiated) {
            findDirectories(projectName: projectName, hints: hints)
        }.value
    }

    /// Paths shown by the runtime event UI. They are safe display suffixes such
    /// as `.../.codex/memories`, not original absolute paths.
    static func workspacePaths(projectName: String, events: [RuntimeEvent]) -> [String] {
        let name = projectName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard isSearchableProject(name) else { return [] }
        return workspaceHints(for: name, events: events)
    }

    private static func isSearchableProject(_ name: String) -> Bool {
        !name.isEmpty && name != "unidentified_project" && name != "multiple_workspaces"
    }

    private static func workspaceHints(for projectName: String, events: [RuntimeEvent]) -> [String] {
        var hints = Set<String>()
        for event in events where event.kind == "client" {
            for path in event.codexMetadata.map({ Array($0.workspaces.keys) }) ?? [] {
                guard pathProjectNames(path).contains(projectName) else { continue }
                hints.insert(path)
            }
        }
        return hints.sorted()
    }

    private static func pathProjectNames(_ path: String) -> Set<String> {
        let parts = normalizedComponents(path)
        guard let last = parts.last else { return [] }
        var names: Set<String> = [last]
        if parts.count >= 2 {
            names.insert("\(parts[parts.count - 2])/\(last)")
        }
        return names
    }

    private static func findDirectories(projectName: String, hints: [String]) -> Outcome {
        let fileManager = FileManager.default
        let home = fileManager.homeDirectoryForCurrentUser
        var paths = Set<String>()

        for hint in hints {
            for url in directCandidates(for: hint, home: home, fileManager: fileManager)
                where matchesProject(url, projectName: projectName, hint: hint, fileManager: fileManager) {
                paths.insert(url.standardizedFileURL.path)
            }
        }

        // A redacted suffix may not be rooted below the home directory (for
        // example a workspace on another volume), so use Spotlight as a
        // read-only fallback. Passing arguments directly avoids shell parsing.
        if paths.isEmpty {
            for url in spotlightCandidates(projectName: projectName, home: home, fileManager: fileManager)
                where matchesProject(url, projectName: projectName, hint: nil, fileManager: fileManager) {
                if hints.isEmpty || hints.contains(where: { suffixMatches(url, hint: $0) }) {
                    paths.insert(url.standardizedFileURL.path)
                }
                if paths.count >= 20 { break }
            }
        }

        guard !paths.isEmpty else { return .unavailable }
        return .matches(Array(paths.sorted().prefix(20)))
    }

    private static func directCandidates(
        for hint: String,
        home: URL,
        fileManager: FileManager
    ) -> [URL] {
        let normalized = hint.replacingOccurrences(of: "\\", with: "/")
        let parts = normalizedComponents(normalized)
        guard !parts.isEmpty else { return [] }
        var candidates: [URL] = []

        if normalized.hasPrefix("/") {
            candidates.append(URL(fileURLWithPath: normalized, isDirectory: true))
        }

        // Sanitized paths retain only their final one or two components. Check
        // the home directory and each direct child as bounded candidate roots;
        // this resolves both `.../.codex/memories` and paths such as
        // `.../claude/rustdesk` under `~/Documents` without a broad disk scan.
        let suffix: ArraySlice<String>
        if let marker = parts.firstIndex(of: "..."), marker + 1 < parts.count {
            suffix = parts[(marker + 1)...]
        } else if !normalized.hasPrefix("/") && !normalized.contains("://") {
            suffix = parts[...]
        } else {
            suffix = []
        }
        if !suffix.isEmpty {
            for root in boundedRoots(home: home, fileManager: fileManager) {
                candidates.append(suffix.reduce(root) { $0.appendingPathComponent($1, isDirectory: true) })
            }
        }

        return candidates.filter { isDirectory($0, fileManager: fileManager) }
    }

    private static func boundedRoots(home: URL, fileManager: FileManager) -> [URL] {
        let children = (try? fileManager.contentsOfDirectory(
            at: home,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsPackageDescendants]
        )) ?? []
        return [home] + children.filter { url in
            (try? url.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true
        }
    }

    private static func spotlightCandidates(
        projectName: String,
        home: URL,
        fileManager: FileManager
    ) -> [URL] {
        let leaf = projectName.split(separator: "/").last.map(String.init) ?? projectName
        guard !leaf.isEmpty,
              fileManager.isExecutableFile(atPath: "/usr/bin/mdfind") else {
            return []
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/mdfind")
        process.arguments = [
            "-onlyin", home.path,
            "kMDItemFSName == '\(spotlightLiteral(leaf))' && kMDItemContentType == 'public.folder'"
        ]
        let output = Pipe()
        process.standardOutput = output
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            process.waitUntilExit()
        } catch {
            return []
        }

        guard let text = String(
            data: output.fileHandleForReading.readDataToEndOfFile(),
            encoding: .utf8
        ) else {
            return []
        }
        return text.split(whereSeparator: \.isNewline).compactMap { line in
            let url = URL(fileURLWithPath: String(line), isDirectory: true)
            return isDirectory(url, fileManager: fileManager) ? url : nil
        }
    }

    private static func spotlightLiteral(_ value: String) -> String {
        value
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "'", with: "\\'")
    }

    private static func matchesProject(
        _ url: URL,
        projectName: String,
        hint: String?,
        fileManager: FileManager
    ) -> Bool {
        guard isDirectory(url, fileManager: fileManager) else { return false }
        let names = pathProjectNames(url.path)
        guard names.contains(projectName) else { return false }
        guard let hint else { return true }
        return suffixMatches(url, hint: hint)
    }

    private static func suffixMatches(_ url: URL, hint: String) -> Bool {
        let candidate = normalizedComponents(url.path)
        let hintParts = normalizedComponents(hint)
        guard !candidate.isEmpty, !hintParts.isEmpty else { return false }
        let suffix: [String]
        if let marker = hintParts.firstIndex(of: "...") {
            suffix = Array(hintParts[(marker + 1)...])
        } else {
            suffix = hintParts
        }
        guard !suffix.isEmpty, candidate.count >= suffix.count else { return false }
        return Array(candidate.suffix(suffix.count)) == suffix
    }

    private static func normalizedComponents(_ path: String) -> [String] {
        path
            .replacingOccurrences(of: "\\", with: "/")
            .split(separator: "/")
            .map(String.init)
            .filter { !$0.isEmpty && $0 != "." && $0 != ".." }
    }

    private static func isDirectory(_ url: URL, fileManager: FileManager) -> Bool {
        var isDirectory: ObjCBool = false
        return fileManager.fileExists(atPath: url.path, isDirectory: &isDirectory)
            && isDirectory.boolValue
    }
}
