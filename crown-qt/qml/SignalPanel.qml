pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Rectangle {
    id: root
    objectName: "signalPanel"

    required property var bridge
    readonly property var channelNames: {
        root.bridge.rev
        return root.bridge.channels()
    }
    readonly property int cleanCount: {
        root.bridge.rev
        let clean = 0
        for (let channel = 0; channel < channelNames.length; channel++) {
            const quality = root.bridge.quality(channel)
            if (quality === "Great" || quality === "Good")
                clean++
        }
        return clean
    }

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
                text: qsTr("SIGNAL")
                color: colors.readout
                font.family: colors.monoFont
                font.pixelSize: 11
                font.weight: Font.DemiBold
                font.letterSpacing: 1.8
            }

            Item { Layout.fillWidth: true }

            Text {
                text: root.channelNames.length === 0
                    ? "—"
                    : root.cleanCount + "/" + root.channelNames.length + " CLEAN"
                color: root.cleanCount === root.channelNames.length && root.channelNames.length > 0
                    ? colors.cyan : colors.muted
                font.family: colors.monoFont
                font.pixelSize: 9
                font.letterSpacing: 0.8
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: colors.faintLine
        }

        Item {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.minimumHeight: 120

            ListView {
                id: qualityList
                anchors.fill: parent
                anchors.topMargin: 8
                anchors.bottomMargin: 8
                model: root.channelNames
                clip: true
                spacing: 2
                boundsBehavior: Flickable.StopAtBounds
                ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

                delegate: Rectangle {
                    id: qualityRow
                    required property int index
                    required property string modelData
                    readonly property string status: {
                        root.bridge.rev
                        return root.bridge.quality(index)
                    }
                    readonly property color statusColor: status === "Great" ? colors.cyan
                        : status === "Good" ? colors.good
                        : status === "Bad" ? colors.warning
                        : status === "NoContact" ? colors.danger : colors.quiet

                    width: qualityList.width
                    height: 38
                    radius: 5
                    color: index % 2 === 0 ? Qt.alpha(colors.instrumentLine, 0.13) : "transparent"

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 9
                        anchors.rightMargin: 9
                        spacing: 9

                        Rectangle {
                            Layout.preferredWidth: 7
                            Layout.preferredHeight: 7
                            radius: 4
                            color: qualityRow.statusColor
                        }

                        Text {
                            text: qualityRow.modelData.toUpperCase()
                            color: colors.readout
                            font.family: colors.monoFont
                            font.pixelSize: 11
                            font.weight: Font.DemiBold
                        }

                        Item { Layout.fillWidth: true }

                        Text {
                            text: qualityRow.status === "NoContact"
                                ? qsTr("NO CONTACT") : qualityRow.status.toUpperCase()
                            color: qualityRow.statusColor
                            font.family: colors.monoFont
                            font.pixelSize: 8
                            font.letterSpacing: 0.5
                        }
                    }
                }
            }

            Column {
                anchors.centerIn: parent
                width: parent.width - 20
                spacing: 7
                visible: root.channelNames.length === 0

                Text {
                    width: parent.width
                    text: qsTr("NO CHANNEL DATA")
                    color: colors.muted
                    font.family: colors.monoFont
                    font.pixelSize: 10
                    font.letterSpacing: 1
                    horizontalAlignment: Text.AlignHCenter
                }

                Text {
                    width: parent.width
                    text: root.bridge.active
                        ? qsTr("Waiting for the device stream")
                        : qsTr("Connect the Crown to inspect contact quality")
                    color: colors.quiet
                    font.family: colors.displayFont
                    font.pixelSize: 11
                    wrapMode: Text.WordWrap
                    horizontalAlignment: Text.AlignHCenter
                }
            }
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 1
            color: colors.faintLine
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.preferredHeight: 42

            Text {
                text: qsTr("DROPPED")
                color: colors.quiet
                font.family: colors.monoFont
                font.pixelSize: 8
                font.letterSpacing: 1
            }

            Item { Layout.fillWidth: true }

            Text {
                text: root.bridge.dropped.toLocaleString(Qt.locale(), "f", 0)
                color: root.bridge.dropped > 0 ? colors.warning : colors.readout
                font.family: colors.monoFont
                font.pixelSize: 15
            }
        }
    }
}
