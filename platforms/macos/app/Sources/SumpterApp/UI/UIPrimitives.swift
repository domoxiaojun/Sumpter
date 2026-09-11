import AppKit
import CoreImage
import SumpterCore
import SwiftUI

/// All AppKit pasteboard writes go through one small boundary so callers never
/// report success before `NSPasteboard.setString` has actually accepted data.
enum PasteboardCopy {
    @MainActor
    static func write(_ text: String, to pasteboard: NSPasteboard = .general) -> Bool {
        pasteboard.clearContents()
        return pasteboard.setString(text, forType: .string)
    }
}

/// SwiftUI's native disclosure label does not make the trailing empty area a
/// reliable hit target. This keeps the familiar disclosure affordance while
/// making the whole 44pt row a keyboard- and VoiceOver-accessible toggle.
struct FullRowDisclosure<Label: View, Content: View>: View {
    @State private var localIsExpanded = false
    private let expandedBinding: Binding<Bool>?
    private var isExpanded: Bool {
        get { expandedBinding?.wrappedValue ?? localIsExpanded }
        nonmutating set {
            if let expandedBinding { expandedBinding.wrappedValue = newValue }
            else { localIsExpanded = newValue }
        }
    }
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private let label: Label
    private let content: Content

    init(
        isExpanded: Binding<Bool>? = nil,
        @ViewBuilder label: () -> Label,
        @ViewBuilder content: () -> Content
    ) {
        self.expandedBinding = isExpanded
        self.label = label()
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                if reduceMotion {
                    isExpanded.toggle()
                } else {
                    withAnimation(.easeOut(duration: 0.18)) { isExpanded.toggle() }
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.semibold))
                        .frame(width: 18, height: 18)
                    label
                    Spacer(minLength: 0)
                }
                .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityValue(isExpanded ? "已展开" : "已收起")
            .accessibilityHint("双击展开或收起")

            if isExpanded {
                content
                    .padding(.leading, 26)
                    .padding(.bottom, 4)
            }
        }
    }
}

enum SettingsSection: String, CaseIterable, Identifiable {
    case run
    case providers
    case modelGroups
    case routing
    case security
    case notifications
    case statistics
    case diagnostics
    case help
    case about

    var id: String { rawValue }

    var title: String {
        switch self {
        case .run: "运行"
        case .providers: "入口库"
        case .modelGroups: "模型组"
        case .routing: "Claude Code 路由"
        case .security: "安全"
        case .notifications: "通知"
        case .statistics: "统计"
        case .diagnostics: "诊断"
        case .help: "帮助"
        case .about: "关于"
        }
    }

    var subtitle: String {
        switch self {
        case .run: "启动状态、监听地址、统计和最近事件。"
        case .providers: "共享上游入口、凭据、模型目录及模型参数。"
        case .modelGroups: "统一地址下的模型范围、组优先级和入口故障切换。"
        case .routing: "Claude Code 内部子请求分流与 effort 覆盖。"
        case .security: "监听、入站认证、入站方言和登录项。"
        case .notifications: "Claude Code、Codex CLI 与 Grok Build hook 统一系统通知。"
        case .statistics: "按请求用途、入口和模型看成功率、延迟与成本。"
        case .diagnostics: "配置路径、最近请求和诊断捕获。"
        case .help: "快速开始、客户端接入、常见问题与安全边界。"
        case .about: "版本、许可证、作者与项目链接。"
        }
    }

    var systemImage: String {
        switch self {
        case .run: "gauge.with.dots.needle.bottom.50percent"
        case .providers: "server.rack"
        case .modelGroups: "square.stack.3d.up"
        case .routing: "arrow.triangle.branch"
        case .security: "lock.shield"
        case .notifications: "bell"
        case .statistics: "chart.bar.xaxis"
        case .diagnostics: "wrench.and.screwdriver"
        case .help: "questionmark.circle"
        case .about: "info.circle"
        }
    }
}

struct SettingsPage<Content: View>: View {
    let title: String
    let subtitle: String
    var maxWidth: CGFloat = 1240
    @ViewBuilder var content: Content
    @Environment(\.sumpterPalette) private var palette
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        GeometryReader { proxy in
            ScrollView {
                HStack(alignment: .top, spacing: 0) {
                    LazyVStack(alignment: .leading, spacing: dynamicTypeSize.isAccessibilitySize ? 18 : SumpterTheme.Layout.panelSpacing) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(title)
                                .font(.title2.weight(.semibold))
                                .textSelection(.enabled)
                            Text(subtitle)
                                .font(.callout)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        content
                    }
                    .frame(maxWidth: maxWidth, alignment: .topLeading)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, proxy.size.width < 760 ? 16 : 24)
                .padding(.vertical, proxy.size.height < 640 ? 16 : 24)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        // Keep the scrolling viewport opaque.  A gradient here is painted
        // behind every scroll pass and adds no information once the panels
        // establish their own semantic surfaces.
        .background(palette.canvas)
    }
}

struct SectionPanel<Content: View>: View {
    let title: String
    var hint: String?
    @ViewBuilder var content: Content
    @Environment(\.sumpterPalette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: SumpterTheme.Layout.panelSpacing) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.headline)
                    if let hint, !hint.isEmpty {
                        Text(hint)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 12)
            }
            Divider()
            content
        }
        .padding(SumpterTheme.Layout.panelPadding)
        // One opaque surface is substantially cheaper to move through an
        // NSScrollView than a clipped gradient stack.  The shared border
        // keeps the same visual separation without an offscreen shadow pass.
        .background(
            RoundedRectangle(cornerRadius: SumpterTheme.Layout.panelRadius, style: .continuous)
                .fill(palette.surface)
        )
        .overlay(
            RoundedRectangle(cornerRadius: SumpterTheme.Layout.panelRadius, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }
}

/// A small, deterministic wrapping layout for metadata badges and compact
/// control groups.  Unlike a fixed `HStack`, it keeps each child at its
/// intrinsic width and starts a new line when the available detail width is
/// too small.  This is intentionally a layout-only change: no child is
/// scaled, so Dynamic Type and keyboard focus retain their native metrics.
struct SumpterWrappingLayout: Layout {
    var horizontalSpacing: CGFloat = 8
    var verticalSpacing: CGFloat = 8

    func sizeThatFits(
        proposal: ProposedViewSize,
        subviews: Subviews,
        cache: inout ()
    ) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var lineWidth: CGFloat = 0
        var lineHeight: CGFloat = 0
        var totalWidth: CGFloat = 0
        var totalHeight: CGFloat = 0
        var hasLine = false

        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            let proposedLineWidth = hasLine ? lineWidth + horizontalSpacing + size.width : size.width
            if hasLine, proposedLineWidth > maxWidth {
                totalWidth = max(totalWidth, lineWidth)
                totalHeight += lineHeight + verticalSpacing
                lineWidth = size.width
                lineHeight = size.height
            } else {
                lineWidth = proposedLineWidth
                lineHeight = max(lineHeight, size.height)
            }
            hasLine = true
        }

        if hasLine {
            totalWidth = max(totalWidth, lineWidth)
            totalHeight += lineHeight
        }
        return CGSize(width: totalWidth.isFinite ? totalWidth : 0, height: totalHeight)
    }

    func placeSubviews(
        in bounds: CGRect,
        proposal: ProposedViewSize,
        subviews: Subviews,
        cache: inout ()
    ) {
        let maxWidth = bounds.width
        var x = bounds.minX
        var y = bounds.minY
        var lineHeight: CGFloat = 0
        var hasLine = false

        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            let proposedLineWidth = hasLine ? x - bounds.minX + horizontalSpacing + size.width : size.width
            if hasLine, proposedLineWidth > maxWidth {
                x = bounds.minX
                y += lineHeight + verticalSpacing
                lineHeight = 0
            }
            if x > bounds.minX {
                x += horizontalSpacing
            }
            subview.place(
                at: CGPoint(x: x, y: y),
                anchor: .topLeading,
                proposal: ProposedViewSize(width: size.width, height: size.height)
            )
            x += size.width
            lineHeight = max(lineHeight, size.height)
            hasLine = true
        }
    }
}

/// Theme surface for native macOS tables.  SwiftUI's default table surface
/// is opaque white with alternating gray rows, which breaks the shared
/// canvas/inset/raised hierarchy and also creates an extra visual boundary
/// inside a SectionPanel.  Keep the native Table for selection, sorting and
/// VoiceOver, but make its scroll surface semantic and stable.
struct SumpterTableSurfaceModifier: ViewModifier {
    @Environment(\.sumpterPalette) private var palette

    func body(content: Content) -> some View {
        content
            .tableStyle(.inset)
            .alternatingRowBackgrounds(.disabled)
            .scrollContentBackground(.hidden)
            // Keep the horizontal affordance visible when a table's column
            // layout is wider than the current detail pane.  The native Table
            // still owns scrolling and selection; this only prevents the
            // scrollbar from disappearing before users discover it.
            .scrollIndicators(.visible, axes: .horizontal)
            .background(palette.inset)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 10, style: .continuous)
                    .stroke(palette.borderSubtle, lineWidth: 0.8)
            }
            // A live SSE update must not animate every row/column change.
            // Selection animations remain owned by the native table control.
            .transaction { transaction in
                transaction.animation = nil
            }
    }
}

extension View {
    func sumpterTableSurface() -> some View {
        modifier(SumpterTableSurfaceModifier())
    }
}

/// Shared pager used by the run and statistics surfaces.  In addition to
/// previous/next controls it exposes an editable page number and the current
/// page-size value, so a large retained history does not require repeated
/// clicks.  The input is intentionally text-backed: macOS accepts an empty
/// edit while the user is replacing the number, then clamps on submit.
struct SumpterPaginationControls: View {
    let page: Int
    let totalPages: Int
    let hasPrevious: Bool
    let hasNext: Bool
    var pageSize: Int?
    var pageSizeOptions: [Int] = AdminWire.RuntimeHistoryPage.allowedPageSizes
    var compact = false
    /// Disable every pager interaction while the requested page is in flight.
    /// Keeping the draft visible avoids a second request and preserves the
    /// user's intended jump target during the short loading window.
    var loading = false
    var onPageChange: ((Int) -> Void)?
    var onPageSizeChange: ((Int) -> Void)?
    @State private var pageDraft: String

    init(
        page: Int,
        totalPages: Int,
        hasPrevious: Bool,
        hasNext: Bool,
        pageSize: Int? = nil,
        pageSizeOptions: [Int] = AdminWire.RuntimeHistoryPage.allowedPageSizes,
        compact: Bool = false,
        loading: Bool = false,
        onPageChange: ((Int) -> Void)? = nil,
        onPageSizeChange: ((Int) -> Void)? = nil
    ) {
        self.page = page
        self.totalPages = totalPages
        self.hasPrevious = hasPrevious
        self.hasNext = hasNext
        self.pageSize = pageSize
        self.pageSizeOptions = pageSizeOptions
        self.compact = compact
        self.loading = loading
        self.onPageChange = onPageChange
        self.onPageSizeChange = onPageSizeChange
        _pageDraft = State(initialValue: String(max(1, page)))
    }

    private var pageCount: Int { max(1, totalPages) }

    var body: some View {
        HStack(spacing: 6) {
            if !compact {
                Button("首页") { onPageChange?(1) }
                    .font(.body.weight(.medium))
                    .frame(minWidth: 64, minHeight: 48)
                    .disabled(!hasPrevious)
                    .help("跳到第一页")
            }
            Button(compact ? "‹" : "上一页") {
                onPageChange?(max(1, page - 1))
            }
            .font(.body.weight(.medium))
            .frame(minWidth: compact ? 48 : 80, minHeight: 48)
            .disabled(!hasPrevious)
            .help("上一页")

            HStack(spacing: 3) {
                Text("第")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                TextField("页", text: $pageDraft)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 56)
                    .frame(minHeight: 38)
                    .multilineTextAlignment(.center)
                    .font(.callout.monospacedDigit())
                    .onSubmit(jumpToDraft)
                    .accessibilityLabel("页码")
                Text("/ \(pageCount)")
                    .font(.callout.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Button("跳转", action: jumpToDraft)
                .font(.body.weight(.medium))
                .frame(minWidth: 64, minHeight: 48)
                .controlSize(.regular)
                .disabled(totalPages == 0)
                .help("跳转到输入的页码")

            Button(compact ? "›" : "下一页") {
                onPageChange?(min(pageCount, page + 1))
            }
            .font(.body.weight(.medium))
            .frame(minWidth: compact ? 48 : 80, minHeight: 48)
            .disabled(!hasNext)
            .help("下一页")
            if !compact {
                Button("末页") { onPageChange?(pageCount) }
                    .font(.body.weight(.medium))
                    .frame(minWidth: 64, minHeight: 48)
                    .disabled(!hasNext)
                    .help("跳到最后一页")
            }

            if let pageSize {
                if let onPageSizeChange {
                    Picker("每页", selection: Binding(
                        get: { pageSize },
                        set: { onPageSizeChange($0) }
                    )) {
                        ForEach(pageSizeOptions, id: \.self) { size in
                            Text("每页 \(size) 条").tag(size)
                        }
                    }
                    .pickerStyle(.menu)
                    .labelsHidden()
                    .font(.body.weight(.medium))
                    .frame(minWidth: compact ? 112 : 124, minHeight: 48)
                    .help("每页显示数量")
                } else {
                    Text("每页 \(pageSize)")
                        .font(.callout.monospacedDigit())
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("每页 \(pageSize) 条")
                }
            }
        }
        // A visible bordered style gives the enlarged hit targets a clear
        // affordance, especially when the pager is moved below a table.
        .buttonStyle(.bordered)
        .controlSize(.regular)
        .frame(minHeight: 48)
        .disabled(loading)
        .opacity(loading ? 0.72 : 1)
        .accessibilityValue(loading ? "正在加载" : "第 \(page) 页，共 \(pageCount) 页")
        .onChange(of: page) { _, value in
            pageDraft = String(max(1, value))
        }
    }

    private func jumpToDraft() {
        let parsed = Int(pageDraft.trimmingCharacters(in: .whitespacesAndNewlines)) ?? page
        let target = min(pageCount, max(1, parsed))
        pageDraft = String(target)
        guard target != page || parsed != page else { return }
        onPageChange?(target)
    }
}

struct StatusBadge: View {
    let text: String
    let systemImage: String
    var color: Color

    var body: some View {
        Label(text, systemImage: systemImage)
            .font(.caption.weight(.semibold))
            .foregroundStyle(color)
            .padding(.horizontal, 9)
            .padding(.vertical, 5)
            .background(color.opacity(0.12), in: Capsule())
            .overlay(Capsule().stroke(color.opacity(0.24), lineWidth: 0.8))
            .accessibilityLabel(text)
    }
}

/// Small, semantic activity cue used for in-flight requests and streaming
/// output.  The visual stays a rotating multi-colour ring, but the rotation is
/// a Core Animation transform on a tiny layer instead of a SwiftUI state
/// animation.  That keeps the compositor animation alive without invalidating
/// the page ViewGraph on every frame.
struct RuntimeActivityIndicator: View {
    var label: String?
    var color: Color = .accentColor
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.sumpterWindowVisible) private var windowVisible
    @Environment(\.sumpterPalette) private var palette

    private var spectrum: [Color] {
        [
            palette.brand,
            palette.info,
            Color.purple,
            palette.warning,
            palette.brand,
        ]
    }

    var body: some View {
        HStack(spacing: 6) {
            RuntimeSpectrumRing(
                colors: spectrum,
                dotColor: color.opacity(0.42),
                isAnimating: !reduceMotion && windowVisible
            )
            .frame(width: 12, height: 12)
            .accessibilityHidden(true)
            if let label, !label.isEmpty {
                Text(label)
            }
        }
        .font(.caption)
        .foregroundStyle(color)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(label ?? "进行中")
    }
}

/// A perimeter glow for an in-flight row. The old left-side breathing rail is
/// intentionally gone; this keeps the live cue around the whole card while
/// moving only a small irregular wave packet along its stroke (no rotating
/// background and no SwiftUI layout invalidation).
struct RuntimeLiveGlowBorder: View {
    var color: Color = .accentColor
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.sumpterWindowVisible) private var windowVisible
    @Environment(\.sumpterPalette) private var palette

    var body: some View {
        RuntimeLiveGlowBorderLayer(
            colors: [
                color.opacity(0.12),
                color,
                palette.info,
                Color.purple,
                color.opacity(0.12),
            ],
            isAnimating: !reduceMotion && windowVisible
        )
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

private struct RuntimeSpectrumRing: NSViewRepresentable {
    let colors: [Color]
    let dotColor: Color
    let isAnimating: Bool

    func makeNSView(context: Context) -> SpectrumRingNSView {
        SpectrumRingNSView()
    }

    func updateNSView(_ nsView: SpectrumRingNSView, context: Context) {
        nsView.update(colors: colors, dotColor: dotColor, isAnimating: isAnimating)
    }
}

private final class SpectrumRingNSView: NSView {
    private let gradientLayer = CAGradientLayer()
    private let ringMask = CAShapeLayer()
    private let dotLayer = CAShapeLayer()
    private var currentAnimating = false

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer = CALayer()
        layer?.addSublayer(gradientLayer)
        layer?.addSublayer(dotLayer)
        gradientLayer.type = .conic
        gradientLayer.startPoint = CGPoint(x: 0.5, y: 0.5)
        gradientLayer.endPoint = CGPoint(x: 1, y: 0.5)
        gradientLayer.mask = ringMask
        ringMask.fillColor = nil
        ringMask.fillRule = .evenOdd
        ringMask.lineWidth = 2
        ringMask.strokeColor = NSColor.white.cgColor
        dotLayer.fillColor = NSColor.white.cgColor
        layer?.isOpaque = false
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override var intrinsicContentSize: NSSize { NSSize(width: 12, height: 12) }

    override func layout() {
        super.layout()
        gradientLayer.frame = bounds
        ringMask.frame = bounds
        let center = CGPoint(x: bounds.midX, y: bounds.midY)
        let outer = max(0, min(bounds.width, bounds.height) / 2 - 1)
        let inner = max(0, outer - 2)
        let path = CGMutablePath()
        path.addEllipse(in: CGRect(
            x: center.x - outer,
            y: center.y - outer,
            width: outer * 2,
            height: outer * 2
        ))
        path.addEllipse(in: CGRect(
            x: center.x - inner,
            y: center.y - inner,
            width: inner * 2,
            height: inner * 2
        ))
        ringMask.path = path
        dotLayer.frame = CGRect(x: center.x - 1.5, y: center.y - 1.5, width: 3, height: 3)
        dotLayer.path = CGPath(ellipseIn: dotLayer.bounds, transform: nil)
    }

    func update(colors: [Color], dotColor: Color, isAnimating: Bool) {
        gradientLayer.colors = colors.map(Self.cgColor)
        dotLayer.fillColor = Self.cgColor(dotColor)
        guard currentAnimating != isAnimating else { return }
        currentAnimating = isAnimating
        if isAnimating {
            let animation = CABasicAnimation(keyPath: "transform.rotation.z")
            animation.fromValue = 0
            animation.toValue = CGFloat.pi * 2
            // The conic gradient's leading colour advances left → right with
            // a positive z rotation in the rendered macOS layer.  Keep this
            // tiny compositor-only cue inexpensive and independent of the
            // surrounding SwiftUI layout.
            animation.duration = 3.2
            animation.repeatCount = .infinity
            animation.timingFunction = CAMediaTimingFunction(name: .linear)
            gradientLayer.add(animation, forKey: "sumpter.spectrum.rotation")
        } else {
            gradientLayer.removeAnimation(forKey: "sumpter.spectrum.rotation")
            gradientLayer.setAffineTransform(.identity)
        }
    }

    /// AppKit's `NSColor(Color)` conversion is annotated `@MainActor` in the
    /// Xcode 16 SDK, while AppKit callbacks such as `update(_:)` are imported
    /// as synchronous/nonisolated.  The callbacks are nevertheless delivered
    /// on the main thread by AppKit; make that invariant explicit at this
    /// boundary instead of leaking actor-isolation errors into the animation
    /// update path.
    private nonisolated static func cgColor(_ color: Color) -> CGColor {
        MainActor.assumeIsolated {
            NSColor(color).usingColorSpace(.deviceRGB)?.cgColor ?? NSColor.white.cgColor
        }
    }
}

private struct RuntimeLiveGlowBorderLayer: NSViewRepresentable {
    let colors: [Color]
    let isAnimating: Bool

    func makeNSView(context: Context) -> LiveGlowBorderNSView {
        LiveGlowBorderNSView()
    }

    func updateNSView(_ nsView: LiveGlowBorderNSView, context: Context) {
        nsView.update(colors: colors, isAnimating: isAnimating)
    }
}

private final class LiveGlowBorderNSView: NSView {
    private let gradientLayer = CAGradientLayer()
    private let glowGradientLayer = CAGradientLayer()
    private let borderMask = CAShapeLayer()
    private let glowMask = CAShapeLayer()
    private var currentAnimating = false
    private var perimeter: CGFloat = 0
    private var animatedPerimeter: CGFloat = 0

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer = CALayer()
        layer?.addSublayer(glowGradientLayer)
        layer?.addSublayer(gradientLayer)
        gradientLayer.type = .conic
        gradientLayer.startPoint = CGPoint(x: 0.5, y: 0.5)
        gradientLayer.endPoint = CGPoint(x: 1, y: 0.5)
        gradientLayer.mask = borderMask
        glowGradientLayer.type = .conic
        glowGradientLayer.startPoint = CGPoint(x: 0.5, y: 0.5)
        glowGradientLayer.endPoint = CGPoint(x: 1, y: 0.5)
        glowGradientLayer.mask = glowMask
        // Both layers are broad and translucent. There is deliberately no
        // crisp core stroke: the live state reads as a small emitted halo,
        // not as a coloured line drawn around the card.
        configureStrokeMask(borderMask, lineWidth: 8.0)
        configureStrokeMask(glowMask, lineWidth: 13.0)
        if let blur = CIFilter(name: "CIGaussianBlur") {
            blur.setValue(1.35, forKey: "inputRadius")
            gradientLayer.filters = [blur]
        }
        if let blur = CIFilter(name: "CIGaussianBlur") {
            blur.setValue(3.6, forKey: "inputRadius")
            glowGradientLayer.filters = [blur]
        }
        gradientLayer.opacity = 0.10
        glowGradientLayer.opacity = 0.08
        layer?.isOpaque = false
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }

    override func layout() {
        super.layout()
        glowGradientLayer.frame = bounds
        gradientLayer.frame = bounds
        borderMask.frame = bounds
        glowMask.frame = bounds
        let outer = bounds.insetBy(dx: 1, dy: 1)
        let radius = min(14, max(0, min(outer.width, outer.height) / 2))
        // Build the perimeter explicitly from top-left → top-right →
        // bottom-right → bottom-left.  This is the visual clockwise order in
        // the macOS window and avoids relying on CGPath's implicit winding.
        let path = CGMutablePath()
        path.move(to: CGPoint(x: outer.minX + radius, y: outer.maxY))
        path.addLine(to: CGPoint(x: outer.maxX - radius, y: outer.maxY))
        path.addArc(
            center: CGPoint(x: outer.maxX - radius, y: outer.maxY - radius),
            radius: radius,
            startAngle: .pi / 2,
            endAngle: 0,
            clockwise: true
        )
        path.addLine(to: CGPoint(x: outer.maxX, y: outer.minY + radius))
        path.addArc(
            center: CGPoint(x: outer.maxX - radius, y: outer.minY + radius),
            radius: radius,
            startAngle: 0,
            endAngle: -.pi / 2,
            clockwise: true
        )
        path.addLine(to: CGPoint(x: outer.minX + radius, y: outer.minY))
        path.addArc(
            center: CGPoint(x: outer.minX + radius, y: outer.minY + radius),
            radius: radius,
            startAngle: -.pi / 2,
            endAngle: -.pi,
            clockwise: true
        )
        path.addLine(to: CGPoint(x: outer.minX, y: outer.maxY - radius))
        path.addArc(
            center: CGPoint(x: outer.minX + radius, y: outer.maxY - radius),
            radius: radius,
            startAngle: .pi,
            endAngle: .pi / 2,
            clockwise: true
        )
        path.closeSubpath()
        borderMask.path = path
        glowMask.path = path

        let straightWidth = max(0, outer.width - radius * 2)
        let straightHeight = max(0, outer.height - radius * 2)
        perimeter = max(1, 2 * (straightWidth + straightHeight) + 2 * .pi * radius)
        // Two uneven dash/gap rhythms overlap into one small wave packet. The long
        // trailing gap localises the activity and the soft masks prevent any
        // crisp rainbow line from appearing around the full card.
        borderMask.lineDashPattern = scaledPattern([
            0.006, 0.009, 0.019, 0.006, 0.029, 0.009, 0.012, 0.910,
        ])
        glowMask.lineDashPattern = scaledPattern([
            0.008, 0.007, 0.014, 0.008, 0.024, 0.008, 0.009, 0.922,
        ])
        if currentAnimating, abs(animatedPerimeter - perimeter) > 0.5 {
            installAnimations()
        }
    }

    func update(colors: [Color], isAnimating: Bool) {
        // Repeat the spectrum around the conic gradient so the uneven wave
        // packet contains a soft multi-colour transition instead of becoming
        // a single solid magenta/blue line at one screen position.
        let cgColors = Self.repeatingSpectrum(colors.map(Self.cgColor))
        gradientLayer.colors = cgColors
        glowGradientLayer.colors = cgColors
        gradientLayer.opacity = isAnimating ? 0.16 : 0.10
        glowGradientLayer.opacity = isAnimating ? 0.19 : 0.08
        guard currentAnimating != isAnimating else { return }
        currentAnimating = isAnimating
        if isAnimating {
            installAnimations()
        } else {
            removeAnimations()
        }
    }

    private func configureStrokeMask(_ mask: CAShapeLayer, lineWidth: CGFloat) {
        mask.fillColor = nil
        mask.strokeColor = NSColor.white.cgColor
        mask.lineWidth = lineWidth
        mask.lineCap = .round
        mask.lineJoin = .round
    }

    private func installAnimations() {
        guard perimeter > 0 else { return }
        removeAnimations()
        animatedPerimeter = perimeter
        borderMask.lineDashPhase = 0
        glowMask.lineDashPhase = 0
        borderMask.add(makePhaseAnimation(), forKey: "sumpter.liveGlow.phase")
        glowMask.add(makePhaseAnimation(), forKey: "sumpter.liveGlow.phase")
        gradientLayer.add(makeBreathingAnimation(), forKey: "sumpter.liveGlow.breathe")
        glowGradientLayer.add(makeBreathingAnimation(), forKey: "sumpter.liveGlow.breathe")
    }

    private func makePhaseAnimation() -> CABasicAnimation {
        let phase = CABasicAnimation(keyPath: "lineDashPhase")
        phase.fromValue = 0
        // The path starts at the upper-left and travels left → right across
        // the top edge. A negative phase advances the packet along that path,
        // which is the visual clockwise direction; positive phase is visibly
        // counter-clockwise in the rendered card.
        phase.toValue = -perimeter
        // Match the Linux rounded-rectangle glow: a calm ten-second lap
        // keeps the perimeter cue visible without turning the row into a
        // high-frequency distraction.
        phase.duration = 10.5
        phase.repeatCount = .infinity
        phase.timingFunction = CAMediaTimingFunction(name: .linear)
        return phase
    }

    private func makeBreathingAnimation() -> CABasicAnimation {
        let breathe = CABasicAnimation(keyPath: "opacity")
        breathe.fromValue = 0.55
        breathe.toValue = 1.0
        breathe.duration = 4.2
        breathe.autoreverses = true
        breathe.repeatCount = .infinity
        breathe.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
        return breathe
    }

    private func removeAnimations() {
        borderMask.removeAnimation(forKey: "sumpter.liveGlow.phase")
        glowMask.removeAnimation(forKey: "sumpter.liveGlow.phase")
        gradientLayer.removeAnimation(forKey: "sumpter.liveGlow.breathe")
        glowGradientLayer.removeAnimation(forKey: "sumpter.liveGlow.breathe")
        animatedPerimeter = 0
    }

    private func scaledPattern(_ fractions: [CGFloat]) -> [NSNumber] {
        fractions.map { NSNumber(value: Double(max(0.5, perimeter * $0))) }
    }

    private nonisolated static func cgColor(_ color: Color) -> CGColor {
        MainActor.assumeIsolated {
            NSColor(color).usingColorSpace(.deviceRGB)?.cgColor ?? NSColor.white.cgColor
        }
    }

    private static func repeatingSpectrum(_ colors: [CGColor], repeats: Int = 7) -> [CGColor] {
        guard colors.count > 1, repeats > 1 else { return colors }
        var result: [CGColor] = []
        result.reserveCapacity((colors.count - 1) * repeats + 1)
        for _ in 0..<repeats {
            result.append(contentsOf: colors.dropLast())
        }
        result.append(colors.last!)
        return result
    }
}

/// 原生 Canvas 近似 Web 的 Paper Mesh Gradient 色彩运动，
/// 两端共用配色、14 点边缘范围和 10 秒呼吸周期。
struct RuntimeLiveSurfaceLight: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.sumpterWindowVisible) private var windowVisible
    @Environment(\.colorScheme) private var colorScheme
    @State private var appeared = false

    var body: some View {
        GeometryReader { proxy in
            if !reduceMotion && windowVisible && appeared {
                TimelineView(.animation(minimumInterval: 1.0 / 24.0)) { timeline in
                    aurora(size: proxy.size, time: timeline.date.timeIntervalSinceReferenceDate)
                }
            } else {
                aurora(size: proxy.size, time: 0)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .onAppear { appeared = true }
        .onDisappear { appeared = false }
    }

    private func aurora(size: CGSize, time: TimeInterval) -> some View {
        let height = max(1, size.height)
        let edge = min(14 / height, 0.5)
        let shoulder = min(4 / height, edge)
        return Canvas { context, canvasSize in
            RuntimeLiveAurora.draw(context: context, size: canvasSize, time: time, light: colorScheme == .light)
        }
        .frame(width: max(1, size.width), height: max(1, size.height))
        .mask {
            LinearGradient(
                stops: [
                    .init(color: .black, location: 0),
                    .init(color: .black.opacity(0.35), location: shoulder),
                    .init(color: .clear, location: edge),
                    .init(color: .clear, location: 1 - edge),
                    .init(color: .black.opacity(0.35), location: 1 - shoulder),
                    .init(color: .black, location: 1),
                ],
                startPoint: .top,
                endPoint: .bottom
            )
        }
    }
}

/// 通过移动径向色点近似 Mesh 的混色；并非 Paper 的 WebGL 着色器。
enum RuntimeLiveAurora {
    static func draw(context: GraphicsContext, size: CGSize, time: TimeInterval, light: Bool) {
        guard size.width > 0, size.height > 0 else { return }
        let breath = 0.26 - 0.04 * cos(time * .pi * 2 / 10)
        let rgb: [(Double, Double, Double)] = light
            ? [(104, 188, 201), (139, 150, 220), (200, 152, 194), (121, 185, 195)]
            : [(120, 197, 210), (156, 168, 232), (210, 166, 206), (137, 201, 207)]
        let colors = rgb.map { Color(red: $0.0 / 255, green: $0.1 / 255, blue: $0.2 / 255) }
        let width = size.width
        let height = size.height
        let phase = (time * 0.075 + 41.5) * 0.5
        // Same independent trajectories as Paper's getPosition(): the points
        // never form a straight sweep or a repeating edge strip.
        for i in 0..<4 {
            let a = Double(i) * 0.37
            let b = 0.6 + (Double(i).truncatingRemainder(dividingBy: 3) / 3) * 0.9
            let c = 0.8 + (Double(i + 1).truncatingRemainder(dividingBy: 4)) / 4
            let x = 0.5 + 0.5 * sin(phase * b + a)
            let y = 0.5 + 0.5 * cos(phase * c + a * 1.5)
            let center = CGPoint(x: x * width, y: y * height)
            let radius = CGSize(width: width * 0.65, height: height * 1.8)
            var layer = context
            layer.translateBy(x: center.x, y: center.y)
            layer.scaleBy(x: radius.width, y: radius.height)
            layer.opacity = breath
            let path = Path(ellipseIn: CGRect(x: -1, y: -1, width: 2, height: 2))
            layer.fill(path, with: .radialGradient(
                Gradient(stops: [
                    .init(color: colors[i].opacity(1), location: 0),
                    .init(color: colors[i].opacity(0.72), location: 0.35),
                    .init(color: .clear, location: 0.86),
                ]),
                center: .zero, startRadius: 0, endRadius: 1
            ))
        }
    }
}

struct RuntimeProgressBar: View {
    let value: Double
    var color: Color = .accentColor
    var label: String?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var clampedValue: Double { min(1, max(0, value)) }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ProgressView(value: clampedValue)
                .progressViewStyle(.linear)
                .tint(color)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.24), value: clampedValue)
            if let label, !label.isEmpty {
                Text(label)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(label ?? "进度")
        .accessibilityValue("\(Int((clampedValue * 100).rounded()))%")
    }
}

struct MetricTile: View {
    let title: String
    let value: String
    var detail: String?
    /// Compact operational summaries use the same visual language with a
    /// smaller footprint, keeping the primary cumulative cards prominent.
    var compact: Bool = false
    /// Optional secondary label displayed at the trailing edge of the detail
    /// row. This keeps the primary value aligned with its peer metric cards.
    var footerAccessory: String? = nil
    var systemImage: String
    /// An optional minimum height lets a group of related KPI cards share one
    /// baseline even when one card has a wrapped detail or an accessory line.
    /// Dynamic Type can still grow the tile beyond this value when needed.
    var minimumHeight: CGFloat? = nil
    @Environment(\.sumpterPalette) private var palette
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    private var accentColor: Color {
        switch systemImage {
        case "checkmark.circle", "checkmark.rectangle", "person.crop.circle.badge.checkmark":
            palette.success
        case "exclamationmark.triangle", "exclamationmark.triangle.fill":
            palette.danger
        case "clock", "timer", "chart.line.uptrend.xyaxis":
            palette.info
        case "yensign.circle":
            palette.brand
        case "arrow.up.doc", "externaldrive.badge.plus", "arrow.up.right.circle":
            palette.warning
        default:
            palette.brand
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: compact ? 6 : 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: systemImage)
                    .font(compact ? .subheadline : .body)
                    .foregroundStyle(accentColor)
                    .frame(width: 18)
                Text(title)
                    .font(compact ? .caption2 : .caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
                Spacer(minLength: 4)
            }
            Text(value)
                .font((compact ? Font.headline : Font.title3).monospacedDigit().weight(.semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.8)
            if (detail?.isEmpty == false) || (footerAccessory?.isEmpty == false) {
                ViewThatFits(in: .horizontal) {
                    metricTileFooter(horizontal: true)
                    metricTileFooter(horizontal: false)
                }
            }
        }
        .padding(compact ? 12 : SumpterTheme.Layout.metricPadding)
        .frame(
            minWidth: compact ? 138 : 150,
            maxWidth: .infinity,
            minHeight: max(
                minimumHeight ?? 0,
                dynamicTypeSize.isAccessibilitySize ? (compact ? 112 : 124) : (compact ? 92 : SumpterTheme.Layout.metricMinimumHeight)
            ),
            alignment: .topLeading
        )
        .background(
            ZStack(alignment: .bottomTrailing) {
                RoundedRectangle(cornerRadius: SumpterTheme.Layout.metricRadius, style: .continuous)
                    .fill(palette.raised)
                // Decorative only: the clipped disk never receives input and
                // stays behind the card's text, matching the Linux cards.
                Circle()
                    .fill(accentColor.opacity(0.10))
                    .frame(width: compact ? 78 : 104, height: compact ? 78 : 104)
                    .offset(x: compact ? 30 : 38, y: compact ? 34 : 48)
            }
            .clipShape(RoundedRectangle(cornerRadius: SumpterTheme.Layout.metricRadius, style: .continuous))
        )
        .overlay(
            RoundedRectangle(cornerRadius: SumpterTheme.Layout.metricRadius, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }

    @ViewBuilder
    private func metricTileFooter(horizontal: Bool) -> some View {
        if horizontal {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                metricTileDetail
                Spacer(minLength: 4)
                metricTileFooterAccessory
            }
        } else {
            VStack(alignment: .leading, spacing: 2) {
                metricTileDetail
                metricTileFooterAccessory
            }
        }
    }

    @ViewBuilder
    private var metricTileDetail: some View {
        if let detail, !detail.isEmpty {
            Text(detail)
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(dynamicTypeSize.isAccessibilitySize ? 3 : 2)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private var metricTileFooterAccessory: some View {
        if let footerAccessory, !footerAccessory.isEmpty {
            Text(footerAccessory)
                .font(.caption2.monospacedDigit().weight(.semibold))
                .foregroundStyle(accentColor)
                .lineLimit(dynamicTypeSize.isAccessibilitySize ? 2 : 1)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

struct EmptyStateView: View {
    let title: String
    var systemImage: String = "tray"
    @Environment(\.sumpterPalette) private var palette

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: systemImage)
                .font(.title2)
                .foregroundStyle(.tertiary)
            Text(title)
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, minHeight: 118)
        .background(palette.inset, in: RoundedRectangle(cornerRadius: SumpterTheme.Layout.panelRadius, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: SumpterTheme.Layout.panelRadius, style: .continuous)
                .stroke(palette.borderSubtle, lineWidth: 0.8)
        )
    }
}

struct InfoRow: View {
    let title: String
    let value: String
    var copyable = false
    /// 弱化显示(占位说明之类的非数据行)。
    var muted = false
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        GridRow {
            Text(title)
                .font(.callout)
                .foregroundStyle(.secondary)
                // Let the grid choose the label column from its longest label.
                // A hard 92pt width clips Chinese labels at Larger Text and
                // forces values into an unnecessarily narrow second column.
                .frame(minWidth: dynamicTypeSize.isAccessibilitySize ? 112 : 92, alignment: .trailing)
            if copyable {
                Text(value.isEmpty ? "-" : value)
                    .font(.callout)
                    .foregroundStyle(muted ? .tertiary : .primary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                Text(value.isEmpty ? "-" : value)
                    .font(.callout)
                    .foregroundStyle(muted ? .tertiary : .primary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }
}

/// 进行中的耗时独立按秒刷新，不受运行页“自动刷新”间隔限制；完成后显示最终 durationMS。
/// 已 accepted 的事件显示「首字节 → 总时长」：流式请求单看总时长分不清上游卡住还是输出长。
struct RuntimeEventDurationText: View {
    let event: RuntimeEvent
    @Environment(\.sumpterWindowVisible) private var windowVisible

    @ViewBuilder
    var body: some View {
        if event.isInFlight && windowVisible {
            TimelineView(.periodic(from: .now, by: 1)) { context in
                Text(RuntimeEventPresentation.durationWithTTFB(
                    ttfbMS: event.ttfbMS,
                    durationMS: event.durationMS,
                    inFlight: true,
                    startedAt: event.timestamp,
                    now: context.date
                ))
            }
        } else {
            Text(RuntimeEventPresentation.durationWithTTFB(
                ttfbMS: event.ttfbMS,
                durationMS: event.durationMS
            ))
        }
    }
}

/// 事件详情里的 streaming / 已持续秒数，同样每秒更新。
struct RuntimeEventStatusInfoRow: View {
    let event: RuntimeEvent
    @Environment(\.sumpterWindowVisible) private var windowVisible

    var body: some View {
        GridRow {
            Text("状态 / 耗时")
                .font(.callout)
                .foregroundStyle(.secondary)
                .frame(width: 92, alignment: .trailing)
            if event.isInFlight && windowVisible {
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(statusLine(now: context.date))
                }
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            } else {
                Text(statusLine(now: Date()))
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func statusLine(now: Date) -> String {
        RuntimeEventPresentation.statusDisplay(
            event.statusCode,
            inFlight: event.isInFlight,
            outcome: event.outcome,
            failureKind: event.failureKind,
            upstreamStatusCode: event.upstreamStatusCode
        )
            + " / " + RuntimeEventPresentation.durationWithTTFB(
                ttfbMS: event.ttfbMS,
                durationMS: event.durationMS,
                inFlight: event.isInFlight,
                startedAt: event.timestamp,
                now: now
            )
            + (RuntimeEventPresentation.isSlowTTFB(event.ttfbMS) ? "(首字节慢)" : "")
            + (event.failover ? " / 故障转移" : "")
    }
}

struct FormLine<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(title)
                .foregroundStyle(.secondary)
                .frame(width: 112, alignment: .trailing)
            content
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct SheetShell<Content: View>: View {
    let title: String
    let primaryTitle: String
    var primaryDisabled = false
    let onCancel: () -> Void
    let onSubmit: () -> Void
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(title)
                .font(.title3.weight(.semibold))
            ScrollView(.vertical) {
                content
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.vertical, 2)
            }
            .frame(maxHeight: 560)
            Divider()
            HStack {
                Spacer()
                Button("取消", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button(primaryTitle, action: onSubmit)
                    .keyboardShortcut(.defaultAction)
                    .disabled(primaryDisabled)
            }
        }
        .padding(20)
        .frame(width: 520)
    }
}

/// 「自动刷新」开关 + 间隔选择,运行页与统计页共用。
/// 关闭时页面冻结当下画面,间隔选择随之禁用。
struct AutoRefreshControl: View {
    @Binding var enabled: Bool
    @Binding var intervalSeconds: Double

    var body: some View {
        HStack(spacing: 6) {
            Toggle("自动刷新", isOn: $enabled)
                .toggleStyle(.checkbox)
            Picker("刷新间隔", selection: $intervalSeconds) {
                ForEach(autoRefreshIntervalChoices, id: \.self) { seconds in
                    Text(Self.intervalLabel(seconds)).tag(seconds)
                }
            }
            .labelsHidden()
            .fixedSize()
            .disabled(!enabled)
            .help("自动刷新间隔")
        }
    }

    private static func intervalLabel(_ seconds: Double) -> String {
        seconds < 1 ? String(format: "%.1f 秒", seconds) : "\(Int(seconds)) 秒"
    }
}

extension Collection {
    var nonEmpty: Bool { !isEmpty }
}

/// 按行数自适应的表格高度:数据少时不留大片空行条纹,多时封顶靠表内滚动。
/// Provider 入口表的拖拽手柄需要更大的命中区域，因此允许调用方传入更舒适的行高。
func adaptiveTableHeight(
    rows: Int,
    min minHeight: CGFloat = 96,
    max maxHeight: CGFloat,
    rowHeight: CGFloat = 28
) -> CGFloat {
    let header: CGFloat = 28
    return Swift.min(maxHeight, Swift.max(minHeight, header + rowHeight * CGFloat(rows) + 8))
}

/// 可显隐的密钥输入框:编辑已有 Key/Token 时可切明文核对,不用盲改。
struct RevealableSecureField: View {
    let placeholder: String
    @Binding var text: String
    @State private var revealed = false

    var body: some View {
        HStack(spacing: 6) {
            Group {
                if revealed {
                    TextField(placeholder, text: $text)
                } else {
                    SecureField(placeholder, text: $text)
                }
            }
            Button {
                revealed.toggle()
            } label: {
                Image(systemName: revealed ? "eye.slash" : "eye")
            }
            .buttonStyle(.borderless)
            .help(revealed ? "隐藏" : "显示")
            .accessibilityLabel(revealed ? "隐藏密钥" : "显示密钥")
        }
    }
}
