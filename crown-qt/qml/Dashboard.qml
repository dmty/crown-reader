pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: root

    required property var bridge
    readonly property bool compact: width < 900
    readonly property real outerMargin: 14
    readonly property real panelGap: 12
    readonly property real signalPanelWidth: 218
    readonly property real metricsPanelWidth: 238
    readonly property real chartChromeWidth: 102
    readonly property real waveformWidth: Math.max(0, compact
        ? width - outerMargin * 2 - 8 - chartChromeWidth
        : width - outerMargin * 2 - signalPanelWidth - metricsPanelWidth
            - panelGap * 2 - chartChromeWidth)

    signal settingsRequested()

    AppPalette { id: colors }

    Rectangle {
        anchors.fill: parent
        color: colors.deepField
    }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: root.outerMargin
        spacing: root.panelGap

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 68
            radius: 12
            color: colors.panel
            border.width: 1
            border.color: colors.instrumentLine

            RowLayout {
                anchors.fill: parent
                anchors.leftMargin: 18
                anchors.rightMargin: 14
                spacing: 12

                Column {
                    Layout.preferredWidth: root.compact ? 108 : 142
                    spacing: -2

                    Text {
                        text: qsTr("CROWN")
                        color: colors.readout
                        font.family: colors.displayFont
                        font.pixelSize: 19
                        font.weight: Font.DemiBold
                        font.letterSpacing: 2.5
                    }

                    Text {
                        visible: !root.compact
                        text: qsTr("NEURAL READER")
                        color: colors.quiet
                        font.family: colors.monoFont
                        font.pixelSize: 7
                        font.letterSpacing: 1.5
                    }
                }

                Rectangle {
                    Layout.preferredWidth: 7
                    Layout.preferredHeight: 7
                    radius: 4
                    color: root.bridge.connection === "Streaming" ? colors.cyan
                        : root.bridge.connection === "Failed" ? colors.danger : colors.warning
                }

                Text {
                    text: root.bridge.connection.toUpperCase()
                    color: colors.readout
                    font.family: colors.monoFont
                    font.pixelSize: 10
                    font.weight: Font.DemiBold
                    font.letterSpacing: 0.8
                }

                Rectangle {
                    visible: !root.compact
                    Layout.preferredWidth: 1
                    Layout.preferredHeight: 24
                    color: colors.faintLine
                }

                Column {
                    visible: !root.compact
                    Layout.maximumWidth: 180
                    spacing: 2

                    Text {
                        text: qsTr("DEVICE")
                        color: colors.quiet
                        font.family: colors.monoFont
                        font.pixelSize: 7
                        font.letterSpacing: 1
                    }

                    Text {
                        width: 170
                        text: root.bridge.deviceid === "" ? qsTr("NOT CONFIGURED") : root.bridge.deviceid
                        color: colors.muted
                        font.family: colors.monoFont
                        font.pixelSize: 9
                        elide: Text.ElideMiddle
                    }
                }

                Item { Layout.fillWidth: true }

                ConsoleButton {
                    text: root.bridge.raw ? qsTr("RAW ON") : qsTr("RAW OFF")
                    accent: colors.cyan
                    enabled: !root.bridge.active
                    Accessible.name: root.bridge.active
                        ? qsTr("Raw EEG choice is locked during this session")
                        : qsTr("Toggle raw EEG for the next session")
                    onClicked: root.bridge.toggleRaw()
                }

                ConsoleButton {
                    visible: !root.bridge.active
                    text: qsTr("CONNECT")
                    accent: colors.cyan
                    prominent: true
                    onClicked: {
                        if (!root.bridge.configured)
                            root.settingsRequested()
                        else
                            root.bridge.start()
                    }
                }

                ConsoleButton {
                    text: root.bridge.recording === "" ? qsTr("●  RECORD") : qsTr("■  STOP")
                    accent: colors.magenta
                    prominent: root.bridge.recording !== ""
                    enabled: root.bridge.ready
                    Accessible.name: root.bridge.recording === "" ? qsTr("Start recording") : qsTr("Stop recording")
                    onClicked: root.bridge.toggleRecording()
                }

                ConsoleButton {
                    text: qsTr("SET")
                    accent: colors.instrumentLine
                    Accessible.name: qsTr("Settings")
                    onClicked: root.settingsRequested()
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: visible ? Math.max(44, errorText.implicitHeight + 20) : 0
            visible: root.bridge.error !== ""
            radius: 8
            color: Qt.alpha(colors.danger, 0.10)
            border.width: 1
            border.color: Qt.alpha(colors.danger, 0.65)

            Text {
                id: errorText
                anchors.left: parent.left
                anchors.right: parent.right
                anchors.verticalCenter: parent.verticalCenter
                anchors.leftMargin: 14
                anchors.rightMargin: 14
                text: qsTr("STREAM ERROR  /  ") + root.bridge.error
                color: colors.danger
                font.family: colors.monoFont
                font.pixelSize: 9
                font.letterSpacing: 0.4
                wrapMode: Text.WordWrap
                verticalAlignment: Text.AlignVCenter
            }
        }

        Loader {
            id: bodyLoader
            Layout.fillWidth: true
            Layout.fillHeight: true
            sourceComponent: root.compact ? compactBody : desktopBody
        }
    }

    Component {
        id: desktopBody

        RowLayout {
            spacing: root.panelGap

            SignalPanel {
                Layout.preferredWidth: root.signalPanelWidth
                Layout.minimumWidth: 190
                Layout.fillHeight: true
                bridge: root.bridge
            }

            WaveformStack {
                id: waveformPanel
                Layout.fillWidth: true
                Layout.fillHeight: true
                Layout.minimumWidth: 380
                bridge: root.bridge
            }

            Metrics {
                Layout.preferredWidth: root.metricsPanelWidth
                Layout.minimumWidth: 210
                Layout.fillHeight: true
                bridge: root.bridge
            }
        }
    }

    Component {
        id: compactBody

        Flickable {
            contentWidth: width
            contentHeight: compactColumn.implicitHeight
            boundsBehavior: Flickable.StopAtBounds
            clip: true
            ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

            ColumnLayout {
                id: compactColumn
                width: parent.width - 8
                spacing: 12

                WaveformStack {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 430
                    bridge: root.bridge
                }

                SignalPanel {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 300
                    bridge: root.bridge
                }

                Metrics {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 410
                    bridge: root.bridge
                }
            }
        }
    }
}
