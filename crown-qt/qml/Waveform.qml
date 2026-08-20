import QtQuick
import QtQuick.Shapes

Item {
    id: root
    required property var bridge
    required property int channel
    property int rev: 0

    implicitHeight: 60

    Rectangle {
        anchors.fill: parent
        color: "#0d1117"
        radius: 4
    }

    Shape {
        anchors.fill: parent
        preferredRendererType: Shape.CurveRenderer

        ShapePath {
            strokeColor: "#4a90d9"
            strokeWidth: 1
            fillColor: "transparent"

            PathPolyline {
                path: (root.rev, root.bridge.waveform(root.channel, root.height))
            }
        }
    }
}
