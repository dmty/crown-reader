pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

Rectangle {
    id: root
    objectName: "metricsPanel"

    required property var bridge
    readonly property bool streaming: bridge.connection === "Streaming"

    radius: 12
    color: colors.panel
    border.width: 1
    border.color: colors.instrumentLine
    clip: true

    AppPalette { id: colors }

    ColumnLayout {
        anchors.fill: parent
        anchors.margins: 16
        spacing: 0

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 40

            Text {
                text: qsTr("STATE")
                color: colors.readout
                font.family: colors.monoFont
                font.pixelSize: 11
                font.weight: Font.DemiBold
                font.letterSpacing: 1.8
            }

            Item { Layout.fillWidth: true }

            Rectangle {
                Layout.preferredWidth: 6
                Layout.preferredHeight: 6
                radius: 3
                color: !root.streaming
                    ? (root.bridge.connection === "Failed" ? colors.danger
                        : root.bridge.connection === "Reconnecting" ? colors.warning : colors.quiet)
                    : root.bridge.staleness < 0 ? colors.quiet
                    : root.bridge.staleness > 10000 ? colors.danger
                    : root.bridge.staleness > 2000 ? colors.warning : colors.cyan
            }

            Text {
                text: !root.streaming
                    ? (root.bridge.connection === "Reconnecting" ? qsTr("RECONNECTING") : qsTr("OFFLINE"))
                    : root.bridge.staleness < 0 ? qsTr("NO DATA")
                    : root.bridge.staleness > 2000
                        ? Math.round(root.bridge.staleness / 1000) + qsTr("S LATE")
                        : qsTr("LIVE")
                color: colors.muted
                font.family: colors.monoFont
                font.pixelSize: 8
                font.letterSpacing: 0.7
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: colors.faintLine
        }

        Repeater {
            model: [
                { label: qsTr("CALM"), key: "calm", accent: colors.cyan },
                { label: qsTr("FOCUS"), key: "focus", accent: colors.magenta }
            ]

            ColumnLayout {
                id: metricGauge
                required property var modelData
                readonly property real value: Math.max(0, Math.min(1,
                    modelData.key === "calm" ? root.bridge.calm : root.bridge.focus))

                Layout.fillWidth: true
                Layout.topMargin: 16
                Layout.bottomMargin: 12
                spacing: 9

                RowLayout {
                    Layout.fillWidth: true

                    Text {
                        text: metricGauge.modelData.label
                        color: colors.muted
                        font.family: colors.monoFont
                        font.pixelSize: 9
                        font.letterSpacing: 1.2
                    }

                    Item { Layout.fillWidth: true }

                    Text {
                        text: Math.round(metricGauge.value * 100) + "%"
                        color: colors.readout
                        font.family: colors.monoFont
                        font.pixelSize: 18
                    }
                }

                Rectangle {
                    Layout.fillWidth: true
                    Layout.preferredHeight: 4
                    radius: 2
                    color: colors.faintLine

                    Rectangle {
                        width: parent.width * metricGauge.value
                        height: parent.height
                        radius: parent.radius
                        color: metricGauge.modelData.accent
                    }
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.topMargin: 4
            Layout.preferredHeight: 1
            color: colors.faintLine
        }

        Text {
            Layout.topMargin: 16
            Layout.bottomMargin: 8
            text: qsTr("BAND POWER")
            color: colors.readout
            font.family: colors.monoFont
            font.pixelSize: 9
            font.weight: Font.DemiBold
            font.letterSpacing: 1.3
        }

        Repeater {
            model: ["delta", "theta", "alpha", "beta", "gamma"]

            RowLayout {
                id: bandRow
                required property string modelData
                Layout.fillWidth: true
                Layout.preferredHeight: 28

                Text {
                    text: bandRow.modelData.toUpperCase()
                    color: colors.quiet
                    font.family: colors.monoFont
                    font.pixelSize: 9
                    font.letterSpacing: 0.6
                }

                Item { Layout.fillWidth: true }

                Text {
                    text: {
                        root.bridge.rev
                        return root.bridge.band(bandRow.modelData).toFixed(3)
                    }
                    color: colors.readout
                    font.family: colors.monoFont
                    font.pixelSize: 11
                }
            }
        }

        Item { Layout.fillHeight: true; Layout.minimumHeight: 8 }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: colors.faintLine
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.topMargin: 12
            spacing: 4

            Text {
                text: root.bridge.recording === "" ? qsTr("SESSION IDLE") : qsTr("RECORDING")
                color: root.bridge.recording === "" ? colors.quiet : colors.magenta
                font.family: colors.monoFont
                font.pixelSize: 8
                font.letterSpacing: 0.9
            }

            Text {
                Layout.fillWidth: true
                visible: root.bridge.recording !== ""
                text: root.bridge.recording
                color: colors.muted
                font.family: colors.monoFont
                font.pixelSize: 8
                elide: Text.ElideMiddle
            }
        }
    }
}
