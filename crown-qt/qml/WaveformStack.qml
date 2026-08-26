pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

Rectangle {
    id: root
    objectName: "waveformPanel"

    required property var bridge
    readonly property var channelNames: {
        root.bridge.rev
        return root.bridge.channels()
    }
    readonly property int channelCount: channelNames.length
    readonly property real headerHeight: 54
    readonly property real footerHeight: 30
    readonly property real rowHeight: Math.max(
        48,
        (height - headerHeight - footerHeight) / Math.max(1, channelCount)
    )
    readonly property real plotWidth: Math.max(0, width - 102)
    readonly property bool waitingForSamples: {
        root.bridge.rev
        return root.bridge.active && root.bridge.raw && root.channelCount > 0
            && root.bridge.waveform(0, 40).length === 0
    }

    radius: 12
    color: colors.panel
    border.width: 1
    border.color: colors.instrumentLine
    clip: true

    AppPalette { id: colors }

    Item {
        id: header
        width: parent.width
        height: root.headerHeight

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 18
            anchors.rightMargin: 18
            spacing: 10

            Text {
                text: qsTr("LIVE EEG")
                color: colors.readout
                font.family: colors.monoFont
                font.pixelSize: 11
                font.weight: Font.DemiBold
                font.letterSpacing: 1.8
            }

            Text {
                text: root.channelCount === 0 ? qsTr("NO CHANNELS")
                    : root.channelCount + qsTr(" CHANNELS")
                color: colors.quiet
                font.family: colors.monoFont
                font.pixelSize: 9
                font.letterSpacing: 0.7
            }

            Item { Layout.fillWidth: true }

            Rectangle {
                Layout.preferredWidth: 6
                Layout.preferredHeight: 6
                radius: 3
                color: root.bridge.active && root.bridge.raw ? colors.cyan : colors.quiet
            }

            Text {
                text: root.bridge.active
                    ? (root.bridge.raw ? qsTr("RAW STREAM") : qsTr("METRICS ONLY"))
                    : (root.bridge.raw ? qsTr("RAW QUEUED") : qsTr("RAW OFF"))
                color: root.bridge.active && root.bridge.raw ? colors.cyan : colors.muted
                font.family: colors.monoFont
                font.pixelSize: 9
                font.letterSpacing: 0.8
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

    Flickable {
        id: traceViewport
        x: 1
        y: root.headerHeight
        width: root.width - 2
        height: Math.max(0, root.height - root.headerHeight - root.footerHeight)
        contentWidth: width
        contentHeight: Math.max(height, traceColumn.height)
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        interactive: contentHeight > height
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        Column {
            id: traceColumn
            width: traceViewport.width
            height: root.channelCount * root.rowHeight

            Repeater {
                model: root.channelNames

                Waveform {
                    required property int index
                    required property string modelData
                    width: traceColumn.width
                    height: root.rowHeight
                    bridge: root.bridge
                    channel: index
                    channelName: modelData
                    accent: colors.channelColors[index % colors.channelColors.length]
                }
            }
        }

        Column {
            anchors.centerIn: parent
            width: Math.min(parent.width - 48, 360)
            spacing: 8
            visible: root.channelCount === 0

            Text {
                width: parent.width
                text: root.bridge.active ? qsTr("WAITING FOR RAW EEG") : qsTr("THE SCOPE IS IDLE")
                color: colors.readout
                font.family: colors.monoFont
                font.pixelSize: 11
                font.letterSpacing: 1.2
                horizontalAlignment: Text.AlignHCenter
            }

            Text {
                width: parent.width
                text: root.bridge.active
                    ? qsTr("No channel metadata has arrived yet.")
                    : qsTr("Enable raw EEG, then connect the Crown to begin.")
                color: colors.muted
                font.family: colors.displayFont
                font.pixelSize: 12
                wrapMode: Text.WordWrap
                horizontalAlignment: Text.AlignHCenter
            }
        }
    }

    Item {
        y: root.height - root.footerHeight
        width: parent.width
        height: root.footerHeight

        Rectangle {
            width: parent.width
            height: 1
            color: colors.faintLine
        }

        Text {
            anchors.left: parent.left
            anchors.leftMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            text: root.waitingForSamples ? qsTr("NO OSC SAMPLES — ENABLE OSC ON THE DEVICE")
                : qsTr("SHARED AMPLITUDE SCALE")
            color: root.waitingForSamples ? colors.warning : colors.quiet
            font.family: colors.monoFont
            font.pixelSize: 8
            font.letterSpacing: 0.7
        }

        Text {
            anchors.right: parent.right
            anchors.rightMargin: 16
            anchors.verticalCenter: parent.verticalCenter
            text: qsTr("NOW")
            color: colors.muted
            font.family: colors.monoFont
            font.pixelSize: 8
            font.letterSpacing: 0.7
        }
    }
}
