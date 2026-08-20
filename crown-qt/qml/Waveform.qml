import QtQuick
import QtQuick.Shapes

Item {
    id: root
    required property var bridge
    required property int channel

    implicitHeight: 60

    Rectangle {
        anchors.fill: parent
        color: "#0d1117"
        radius: 4
    }

    Shape {
        anchors.fill: parent
        // GeometryRenderer, not CurveRenderer: this path is a plain
        // polyline (min/max zigzag, no curves), and CurveRenderer
        // re-triangulates on the UI thread on every repaint — needless cost
        // at ~1800 points/channel and 30Hz.
        preferredRendererType: Shape.GeometryRenderer

        ShapePath {
            strokeColor: "#4a90d9"
            strokeWidth: 1
            fillColor: "transparent"

            PathPolyline {
                path: {
                    root.bridge.rev; // re-read on every tick bump; waveform() is an invokable, not a bound property
                    return root.bridge.waveform(root.channel, root.height);
                }
            }
        }
    }
}
