import QtQuick
import QtQuick.Controls.Basic

Button {
    id: root

    property color accent: colors.cyan
    property bool prominent: false

    implicitWidth: Math.max(72, contentItem.implicitWidth + 28)
    implicitHeight: 36
    leftPadding: 14
    rightPadding: 14

    AppPalette { id: colors }

    contentItem: Text {
        text: root.text
        color: !root.enabled
            ? colors.quiet
            : (root.prominent ? colors.deepField : colors.readout)
        font.family: colors.monoFont
        font.pixelSize: 11
        font.weight: Font.DemiBold
        font.letterSpacing: 0.8
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    background: Rectangle {
        radius: 8
        color: !root.enabled
            ? colors.faintLine
            : (root.prominent
                ? root.accent
                : (root.down ? Qt.alpha(root.accent, 0.18)
                    : root.hovered ? Qt.alpha(root.accent, 0.10) : "transparent"))
        border.width: root.visualFocus ? 2 : 1
        border.color: root.visualFocus ? colors.readout
            : (!root.enabled ? colors.instrumentLine : root.accent)
    }
}
