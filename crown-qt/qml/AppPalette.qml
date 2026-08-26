import QtQuick

QtObject {
    readonly property color deepField: "#050817"
    readonly property color panel: "#0A1026"
    readonly property color panelRaised: "#0D1530"
    readonly property color instrumentLine: "#24345D"
    readonly property color faintLine: "#141E3B"
    readonly property color readout: "#E7ECFF"
    readonly property color muted: "#8E9ABD"
    readonly property color quiet: "#606D92"
    readonly property color cyan: "#4FD6E5"
    readonly property color magenta: "#FF3D95"
    readonly property color good: "#72D6A4"
    readonly property color warning: "#F2B45B"
    readonly property color danger: "#FF6B7D"

    readonly property var channelColors: [
        "#FF6B9F", "#FF9F51", "#F2CF52", "#65D98A",
        "#4FD6E5", "#45B8FF", "#A985FF", "#E86ED0"
    ]

    // Prefer the planned faces when they are installed, with native desktop
    // fallbacks that preserve the same geometric/technical character.
    readonly property string displayFont: Qt.platform.os === "osx" ? "Avenir Next" : "Space Grotesk"
    readonly property string monoFont: Qt.platform.os === "osx" ? "Menlo" : "IBM Plex Mono"
}
