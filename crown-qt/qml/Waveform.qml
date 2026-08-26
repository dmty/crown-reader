pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Shapes

Item {
    id: root

    required property var bridge
    required property int channel
    required property string channelName
    required property color accent

    readonly property real labelWidth: 64
    readonly property real traceInset: 12

    implicitHeight: 58
    clip: true

    AppPalette { id: colors }

    Rectangle {
        anchors.fill: parent
        color: root.channel % 2 === 0 ? Qt.alpha(colors.instrumentLine, 0.055) : "transparent"
    }

    Rectangle {
        x: 0
        width: 3
        height: parent.height
        color: root.accent
        opacity: 0.85
    }

    Text {
        x: 14
        width: root.labelWidth - 18
        anchors.verticalCenter: parent.verticalCenter
        text: root.channelName.toUpperCase()
        color: root.accent
        font.family: colors.monoFont
        font.pixelSize: 11
        font.weight: Font.DemiBold
        horizontalAlignment: Text.AlignLeft
    }

    Item {
        id: trace
        x: root.labelWidth
        width: Math.max(0, root.width - root.labelWidth - 12)
        height: root.height
        clip: true

        Repeater {
            model: 4

            Rectangle {
                required property int index
                x: Math.round((trace.width - 1) * index / 3)
                width: 1
                height: trace.height
                color: colors.faintLine
            }
        }

        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width
            height: 1
            color: colors.instrumentLine
            opacity: 0.65
        }

        Shape {
            id: traceShape
            x: root.traceInset
            y: 6
            width: Math.max(0, parent.width - root.traceInset * 2)
            height: Math.max(0, parent.height - 12)
            clip: true

            // GeometryRenderer avoids rebuilding a curve mesh at stream rate;
            // the bridge already provides the intended min/max polyline.
            preferredRendererType: Shape.GeometryRenderer

            ShapePath {
                strokeColor: root.accent
                strokeWidth: 1.15
                fillColor: "transparent"

                PathPolyline {
                    path: {
                        root.bridge.rev
                        return root.bridge.waveform(root.channel, traceShape.height)
                    }
                }
            }
        }
    }

    Rectangle {
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: 1
        color: colors.faintLine
    }
}
