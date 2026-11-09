import SwiftUI

/// The prototype's single linear icon set: 24 × 24, 1.6 pt stroke, round caps.
/// Drawn rather than borrowed so the weight matches in both modes.
nonisolated enum SeIcon: String, CaseIterable, Sendable {
    case hosts
    case terminal
    case branch
    case clock
    case folder
    case gear
    case search
    case sidebar
    case plus
    case chevron
    case chevronDown
    case back
    case up
    case close
    case file
    case check
    case play
    case refresh
    case lock
    case sun
    case moon
    case scan
    case link
    case arrowRight
    case more
    case chat
}

struct SeIconView: View {
    let icon: SeIcon
    var size: CGFloat = 18
    var stroke: CGFloat = 1.6
    var color: Color?

    @Environment(\.sePalette) private var palette

    var body: some View {
        SeIconShape(icon: icon)
            .stroke(
                color ?? palette.inkSecondary,
                style: StrokeStyle(lineWidth: stroke, lineCap: .round, lineJoin: .round)
            )
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }
}

nonisolated struct SeIconShape: Shape {
    let icon: SeIcon

    nonisolated func path(in rect: CGRect) -> Path {
        let s = min(rect.width, rect.height) / 24
        var path = Path()
        func p(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
            CGPoint(x: rect.minX + x * s, y: rect.minY + y * s)
        }
        func move(_ x: CGFloat, _ y: CGFloat) { path.move(to: p(x, y)) }
        func line(_ x: CGFloat, _ y: CGFloat) { path.addLine(to: p(x, y)) }
        func circle(_ x: CGFloat, _ y: CGFloat, _ r: CGFloat) {
            path.addEllipse(in: CGRect(x: rect.minX + (x - r) * s, y: rect.minY + (y - r) * s,
                                       width: r * 2 * s, height: r * 2 * s))
        }

        switch icon {
        case .hosts:
            move(12, 3); line(21, 7.5); line(21, 16.5); line(12, 21); line(3, 16.5); line(3, 7.5); path.closeSubpath()
            move(3, 7.5); line(12, 12); line(21, 7.5)
            move(12, 12); line(12, 21)
        case .terminal:
            path.addRoundedRect(in: CGRect(x: rect.minX + 3 * s, y: rect.minY + 4 * s,
                                           width: 18 * s, height: 16 * s),
                                cornerSize: CGSize(width: 2.5 * s, height: 2.5 * s))
            move(7, 10); line(10, 13); line(7, 16)
            move(12.5, 16.5); line(17, 16.5)
        case .branch:
            circle(6, 5, 2)
            circle(6, 19, 2)
            circle(18, 8, 2)
            move(6, 7); line(6, 17)
            move(6, 11); line(6, 12)
            path.addQuadCurve(to: p(18, 10), control: p(11, 15))
        case .clock:
            circle(12, 12, 8.5)
            move(12, 7.2); line(12, 12); line(15.8, 14)
        case .folder:
            move(3, 8)
            line(3, 6.5)
            path.addQuadCurve(to: p(5, 4.5), control: p(3, 4.5))
            line(9.5, 4.5); line(11.5, 6.5); line(19, 6.5)
            path.addQuadCurve(to: p(21, 8.5), control: p(21, 6.5))
            line(21, 17.5)
            path.addQuadCurve(to: p(19, 19.5), control: p(21, 19.5))
            line(5, 19.5)
            path.addQuadCurve(to: p(3, 17.5), control: p(3, 19.5))
            path.closeSubpath()
        case .gear:
            circle(12, 12, 3.2)
            for i in 0..<8 {
                let a = Double(i) * .pi / 4
                let inner: CGFloat = 5.4
                let outer: CGFloat = 8.4
                move(12 + CGFloat(cos(a)) * inner, 12 + CGFloat(sin(a)) * inner)
                line(12 + CGFloat(cos(a)) * outer, 12 + CGFloat(sin(a)) * outer)
            }
        case .search:
            circle(11, 11, 6.5)
            move(15.8, 15.8); line(21, 21)
        case .sidebar:
            path.addRoundedRect(in: CGRect(x: rect.minX + 3 * s, y: rect.minY + 4 * s,
                                           width: 18 * s, height: 16 * s),
                                cornerSize: CGSize(width: 2 * s, height: 2 * s))
            move(9, 4); line(9, 20)
        case .plus:
            move(12, 5); line(12, 19)
            move(5, 12); line(19, 12)
        case .chevron:
            move(9, 6); line(15, 12); line(9, 18)
        case .chevronDown:
            move(6, 9); line(12, 15); line(18, 9)
        case .back:
            move(15, 5); line(8, 12); line(15, 19)
        case .up:
            move(12, 19); line(12, 5)
            move(6, 11); line(12, 5); line(18, 11)
        case .close:
            move(6, 6); line(18, 18)
            move(18, 6); line(6, 18)
        case .file:
            move(6, 3); line(14, 3); line(18, 7); line(18, 21); line(6, 21); path.closeSubpath()
            move(14, 3); line(14, 7); line(18, 7)
        case .check:
            move(5, 12); line(10, 17); line(19, 7)
        case .play:
            move(8, 5); line(19, 12); line(8, 19); path.closeSubpath()
        case .refresh:
            path.addArc(center: p(12, 12), radius: 8 * s,
                        startAngle: .degrees(-55), endAngle: .degrees(235), clockwise: false)
            move(13, 3.2); line(17.4, 6.2); line(13.2, 8.4)
        case .lock:
            path.addRoundedRect(in: CGRect(x: rect.minX + 5 * s, y: rect.minY + 10 * s,
                                           width: 14 * s, height: 10.5 * s),
                                cornerSize: CGSize(width: 2 * s, height: 2 * s))
            move(8, 10); line(8, 7.5)
            path.addArc(center: p(12, 7.5), radius: 4 * s,
                        startAngle: .degrees(180), endAngle: .degrees(0), clockwise: false)
            move(12, 14); line(12, 16.5)
        case .sun:
            circle(12, 12, 4)
            for i in 0..<8 {
                let a = Double(i) * .pi / 4
                move(12 + CGFloat(cos(a)) * 6.2, 12 + CGFloat(sin(a)) * 6.2)
                line(12 + CGFloat(cos(a)) * 8.6, 12 + CGFloat(sin(a)) * 8.6)
            }
        case .moon:
            move(20, 14.6)
            path.addCurve(to: p(9.4, 4), control1: p(20, 8), control2: p(15, 3.2))
            path.addCurve(to: p(20, 14.6), control1: p(9.2, 13.4), control2: p(13.8, 18.4))
            path.closeSubpath()
        case .scan:
            move(4, 9); line(4, 4); line(9, 4)
            move(15, 4); line(20, 4); line(20, 9)
            move(20, 15); line(20, 20); line(15, 20)
            move(9, 20); line(4, 20); line(4, 15)
        case .link:
            move(10, 14)
            path.addQuadCurve(to: p(14, 10), control: p(12, 12))
            move(9.5, 8.5)
            path.addQuadCurve(to: p(6.5, 8.5), control: p(8, 7))
            path.addQuadCurve(to: p(6.5, 15.5), control: p(4.5, 11.8))
            line(8.5, 17.5)
            path.addQuadCurve(to: p(11.5, 17.5), control: p(10, 19))
            move(14.5, 15.5)
            path.addQuadCurve(to: p(17.5, 15.5), control: p(16, 17))
            path.addQuadCurve(to: p(17.5, 8.5), control: p(19.5, 12.2))
            line(15.5, 6.5)
            path.addQuadCurve(to: p(12.5, 6.5), control: p(14, 5))
        case .arrowRight:
            move(4, 12); line(20, 12)
            move(14, 6); line(20, 12); line(14, 18)
        case .more:
            circle(5, 12, 1.2)
            circle(12, 12, 1.2)
            circle(19, 12, 1.2)
        case .chat:
            path.addRoundedRect(in: CGRect(x: rect.minX + 3 * s, y: rect.minY + 4 * s,
                                           width: 18 * s, height: 13.5 * s),
                                cornerSize: CGSize(width: 3 * s, height: 3 * s))
            move(8, 17.5); line(7, 21); line(12, 17.5)
        }
        return path
    }
}

/// Small filled status dot used by the strips and rows.
struct SeNeedDot: View {
    enum Kind: Sendable { case live, wait, fail, off, idle }

    let kind: Kind
    var size: CGFloat = 6

    @Environment(\.sePalette) private var palette

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .accessibilityHidden(true)
    }

    private var color: Color {
        switch kind {
        case .live: palette.live
        case .wait: palette.warning
        case .fail: palette.danger
        case .off: palette.inkTertiary
        case .idle: palette.inkTertiary
        }
    }
}
