import Foundation

/// 表格多选场景下的选择规则（纯逻辑，供设置页各表格复用）。
///
/// 规则对齐旧 Python UI：编辑 / 详情 / 上移 / 下移必须「严格选中一项」；
/// 删除支持多选；配置变更后无效 selection 自动清理，某些表格清空后回落到第一项。
public enum TableSelection {
    /// 仅当恰好选中一项且该 ID 存在于 `rows` 时返回对应行，否则 nil。
    public static func singleSelected<Row: Identifiable>(_ rows: [Row], selection: Set<String>) -> Row?
    where Row.ID == String {
        guard selection.count == 1, let id = selection.first else {
            return nil
        }
        return rows.first { $0.id == id }
    }

    /// 剔除 `validIDs` 之外的选择；若清空且 `selectFirst`，回落到排序后的第一个有效 ID。
    public static func sanitize(_ selection: Set<String>, validIDs: Set<String>, selectFirst: Bool) -> Set<String> {
        var result = selection.filter { validIDs.contains($0) }
        if result.isEmpty, selectFirst, let first = validIDs.sorted().first {
            result = [first]
        }
        return result
    }

    /// 选中项能否按 `direction`(-1 上移 / +1 下移) 移动：必须已选中且目标下标在界内。
    public static func canMove(id: String?, orderedIDs: [String], direction: Int) -> Bool {
        guard let id, let index = orderedIDs.firstIndex(of: id) else {
            return false
        }
        return orderedIDs.indices.contains(index + direction)
    }

    /// 多选整块移动：选中的每一行按 `direction` 各挪一步；抵边的行原地不动，
    /// 紧随其后的选中行会「顶」在它后面（原生 List 拖拽的语义）。返回移动后的完整顺序。
    public static func moved(ids orderedIDs: [String], selection: Set<String>, direction: Int) -> [String] {
        guard direction == -1 || direction == 1, !selection.isEmpty else {
            return orderedIDs
        }
        var result = orderedIDs
        let indices = result.indices.filter { selection.contains(result[$0]) }
        if direction == -1 {
            var barrier = -1
            for index in indices {
                if index - 1 > barrier {
                    result.swapAt(index, index - 1)
                    barrier = index - 1
                } else {
                    barrier = index
                }
            }
        } else {
            var barrier = result.count
            for index in indices.reversed() {
                if index + 1 < barrier {
                    result.swapAt(index, index + 1)
                    barrier = index + 1
                } else {
                    barrier = index
                }
            }
        }
        return result
    }

    /// 多选整块移动是否会产生变化（全部抵边或选择无效时为 false，移动按钮据此禁用）。
    public static func canMove(selection: Set<String>, orderedIDs: [String], direction: Int) -> Bool {
        moved(ids: orderedIDs, selection: selection, direction: direction) != orderedIDs
    }

    /// 将单个 ID 插入到移动后的目标下标。`targetIndex` 是移除源元素之后
    /// 的下标，因此调用方可直接把拖拽目标换算成“放在该行之前/之后”。
    /// 找不到 ID 时保持原顺序；目标下标会被安全限制在数组范围内。
    public static func moved(id: String, orderedIDs: [String], toIndex targetIndex: Int) -> [String] {
        guard let sourceIndex = orderedIDs.firstIndex(of: id) else { return orderedIDs }
        var result = orderedIDs
        let value = result.remove(at: sourceIndex)
        let target = min(max(0, targetIndex), result.count)
        result.insert(value, at: target)
        return result
    }
}
