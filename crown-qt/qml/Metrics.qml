import QtQuick
import QtQuick.Layouts

ColumnLayout {
    id: root
    required property var bridge

    spacing: 10

    RowLayout {
        spacing: 24
        Repeater {
            model: ["calm", "focus"]
            ColumnLayout {
                required property string modelData
                Text { text: modelData; font.pixelSize: 12; opacity: 0.6 }
                Rectangle {
                    width: 120; height: 14; radius: 7; color: "#22000000"
                    Rectangle {
                        height: parent.height; radius: 7
                        color: modelData === "calm" ? "#4a90d9" : "#d98a4a"
                        width: parent.width * (modelData === "calm" ? root.bridge.calm : root.bridge.focus)
                    }
                }
            }
        }
    }

    GridLayout {
        columns: 5
        Repeater {
            model: ["delta", "theta", "alpha", "beta", "gamma"]
            ColumnLayout {
                required property string modelData
                Text { text: modelData; font.pixelSize: 12; opacity: 0.6 }
                Text {
                    text: {
                        root.bridge.rev; // re-read on every tick bump; band() is an invokable, not a bound property
                        return root.bridge.band(modelData).toFixed(3);
                    }
                    font.family: "Menlo"
                }
            }
        }
    }

    // `Layout.fillWidth` only takes effect because `root`'s own width is
    // bound by whoever instantiates this component (see main.qml): without
    // that, Flow has nothing to wrap against and just grows wide enough to
    // fit every tile on one line, pushing tiles for channels above what fits
    // off-screen instead of onto a second row.
    Flow {
        Layout.fillWidth: true
        spacing: 8
        Repeater {
            model: {
                root.bridge.rev; // re-read on every tick bump; channels() is an invokable, not a bound property
                return root.bridge.channels();
            }
            Rectangle {
                required property int index
                required property string modelData
                width: 78; height: 26; radius: 4
                color: {
                    root.bridge.rev; // re-read on every tick bump; quality() is an invokable, not a bound property
                    const q = root.bridge.quality(index);
                    if (q === "Great") return "#3fa34d";
                    if (q === "Good") return "#8ab661";
                    if (q === "Bad") return "#d98a4a";
                    if (q === "NoContact") return "#c04a4a";
                    return "#999999";
                }
                Text {
                    anchors.centerIn: parent
                    text: modelData
                    color: "white"
                    font.pixelSize: 11
                }
            }
        }
    }

    Text {
        text: "dropped: " + root.bridge.dropped
        font.pixelSize: 10
        opacity: 0.4
    }
}
