pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts

Page {
    id: root

    required property var bridge
    signal saved()
    signal cancelled()

    AppPalette { id: colors }

    function selectStoredDevice() {
        for (let index = 0; index < deviceBox.count; index++) {
            if (bridge.deviceIdAt(index) === bridge.deviceid) {
                deviceBox.currentIndex = index
                return
            }
        }
        if (deviceBox.count > 0)
            deviceBox.currentIndex = 0
    }

    function load() {
        emailField.text = bridge.email
        passwordField.text = ""
        selectStoredDevice()
    }

    background: Rectangle { color: colors.deepField }

    header: Rectangle {
        height: 70
        color: colors.panel
        border.width: 1
        border.color: colors.instrumentLine

        RowLayout {
            anchors.fill: parent
            anchors.leftMargin: 22
            anchors.rightMargin: 22

            Column {
                spacing: 1

                Text {
                    text: qsTr("DEVICE SETUP")
                    color: colors.readout
                    font.family: colors.displayFont
                    font.pixelSize: 18
                    font.weight: Font.DemiBold
                    font.letterSpacing: 1.5
                }

                Text {
                    text: qsTr("Neurosity account and Crown selection")
                    color: colors.muted
                    font.family: colors.displayFont
                    font.pixelSize: 11
                }
            }

            Item { Layout.fillWidth: true }

            ConsoleButton {
                text: qsTr("CLOSE")
                accent: colors.instrumentLine
                onClicked: {
                    passwordField.text = ""
                    root.cancelled()
                }
            }
        }
    }

    Flickable {
        anchors.fill: parent
        contentWidth: width
        contentHeight: settingsColumn.implicitHeight + 64
        boundsBehavior: Flickable.StopAtBounds
        clip: true
        ScrollBar.vertical: ScrollBar { policy: ScrollBar.AsNeeded }

        ColumnLayout {
            id: settingsColumn
            width: Math.min(parent.width - 40, 720)
            anchors.horizontalCenter: parent.horizontalCenter
            y: 30
            spacing: 16

            Text {
                Layout.fillWidth: true
                text: qsTr("Connect your account")
                color: colors.readout
                font.family: colors.displayFont
                font.pixelSize: 24
                font.weight: Font.Medium
            }

            Text {
                Layout.fillWidth: true
                text: qsTr("Credentials stay in the system keyring. Load your claimed devices, choose the Crown you want to monitor, then save.")
                color: colors.muted
                font.family: colors.displayFont
                font.pixelSize: 13
                lineHeight: 1.35
                wrapMode: Text.WordWrap
            }

            Rectangle {
                Layout.fillWidth: true
                Layout.topMargin: 6
                Layout.preferredHeight: formLayout.implicitHeight + 40
                radius: 12
                color: colors.panel
                border.width: 1
                border.color: colors.instrumentLine

                ColumnLayout {
                    id: formLayout
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    anchors.margins: 20
                    spacing: 9

                    Text {
                        text: qsTr("EMAIL")
                        color: colors.muted
                        font.family: colors.monoFont
                        font.pixelSize: 9
                        font.letterSpacing: 1.1
                    }

                    TextField {
                        id: emailField
                        Layout.fillWidth: true
                        implicitHeight: 42
                        color: colors.readout
                        placeholderText: qsTr("you@example.com")
                        font.family: colors.displayFont
                        Accessible.name: qsTr("Email")
                        background: Rectangle {
                            radius: 7
                            color: colors.panelRaised
                            border.width: emailField.activeFocus ? 2 : 1
                            border.color: emailField.activeFocus ? colors.cyan : colors.instrumentLine
                        }
                    }

                    Text {
                        Layout.topMargin: 7
                        text: qsTr("PASSWORD")
                        color: colors.muted
                        font.family: colors.monoFont
                        font.pixelSize: 9
                        font.letterSpacing: 1.1
                    }

                    TextField {
                        id: passwordField
                        Layout.fillWidth: true
                        implicitHeight: 42
                        echoMode: TextInput.Password
                        color: colors.readout
                        placeholderText: qsTr("Neurosity password")
                        font.family: colors.displayFont
                        Accessible.name: qsTr("Password")
                        background: Rectangle {
                            radius: 7
                            color: colors.panelRaised
                            border.width: passwordField.activeFocus ? 2 : 1
                            border.color: passwordField.activeFocus ? colors.cyan : colors.instrumentLine
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 7

                        Text {
                            text: qsTr("DEVICE")
                            color: colors.muted
                            font.family: colors.monoFont
                            font.pixelSize: 9
                            font.letterSpacing: 1.1
                        }

                        Item { Layout.fillWidth: true }

                        Text {
                            text: deviceBox.count === 0 ? qsTr("LOAD DEVICES TO CHOOSE")
                                : deviceBox.count + qsTr(" AVAILABLE")
                            color: colors.quiet
                            font.family: colors.monoFont
                            font.pixelSize: 8
                            font.letterSpacing: 0.6
                        }
                    }

                    ComboBox {
                        id: deviceBox
                        Layout.fillWidth: true
                        implicitHeight: 42
                        model: root.bridge.devicelabels
                        enabled: count > 0
                        font.family: colors.displayFont
                        Accessible.name: qsTr("Device")

                        contentItem: Text {
                            leftPadding: 14
                            rightPadding: 38
                            text: deviceBox.displayText
                            color: deviceBox.enabled ? colors.readout : colors.quiet
                            font: deviceBox.font
                            verticalAlignment: Text.AlignVCenter
                            elide: Text.ElideRight
                        }

                        indicator: Text {
                            x: deviceBox.width - width - 15
                            y: (deviceBox.height - height) / 2 - 1
                            text: "⌄"
                            color: deviceBox.enabled ? colors.muted : colors.quiet
                            font.family: colors.displayFont
                            font.pixelSize: 18
                        }

                        background: Rectangle {
                            radius: 7
                            color: colors.panelRaised
                            border.width: deviceBox.activeFocus ? 2 : 1
                            border.color: deviceBox.activeFocus ? colors.cyan : colors.instrumentLine
                        }

                        delegate: ItemDelegate {
                            id: deviceDelegate
                            required property int index
                            required property string modelData
                            width: deviceBox.width - 8
                            text: modelData
                            highlighted: deviceBox.highlightedIndex === index
                            contentItem: Text {
                                text: deviceDelegate.text
                                color: colors.readout
                                font.family: colors.displayFont
                                font.pixelSize: 12
                                verticalAlignment: Text.AlignVCenter
                            }
                            background: Rectangle {
                                radius: 5
                                color: deviceDelegate.highlighted
                                    ? Qt.alpha(colors.cyan, 0.12) : "transparent"
                            }
                        }

                        popup: Popup {
                            y: deviceBox.height + 4
                            width: deviceBox.width
                            implicitHeight: Math.min(contentItem.implicitHeight + 8, 240)
                            padding: 4

                            contentItem: ListView {
                                clip: true
                                implicitHeight: contentHeight
                                model: deviceBox.popup.visible ? deviceBox.delegateModel : null
                                currentIndex: deviceBox.highlightedIndex
                                ScrollIndicator.vertical: ScrollIndicator { }
                            }

                            background: Rectangle {
                                radius: 7
                                color: colors.panelRaised
                                border.width: 1
                                border.color: colors.instrumentLine
                            }
                        }
                    }

                    RowLayout {
                        Layout.fillWidth: true
                        Layout.topMargin: 9
                        spacing: 10

                        ConsoleButton {
                            text: qsTr("LOAD DEVICES")
                            accent: colors.cyan
                            onClicked: {
                                if (root.bridge.listDevices(emailField.text, passwordField.text))
                                    root.selectStoredDevice()
                            }
                        }

                        Item { Layout.fillWidth: true }

                        ConsoleButton {
                            text: qsTr("CLEAR")
                            accent: colors.danger
                            onClicked: clearDialog.open()
                        }

                        ConsoleButton {
                            text: qsTr("SAVE")
                            accent: colors.cyan
                            prominent: true
                            enabled: deviceBox.count > 0
                            onClicked: {
                                const deviceId = root.bridge.deviceIdAt(deviceBox.currentIndex)
                                if (root.bridge.savePasswordAuth(emailField.text, passwordField.text, deviceId)) {
                                    passwordField.text = ""
                                    root.saved()
                                }
                            }
                        }
                    }
                }
            }

            Text {
                Layout.fillWidth: true
                visible: root.bridge.settingserror !== ""
                text: root.bridge.settingserror
                color: colors.danger
                font.family: colors.monoFont
                font.pixelSize: 10
                lineHeight: 1.25
                wrapMode: Text.WordWrap
            }
        }
    }

    Dialog {
        id: clearDialog
        parent: Overlay.overlay
        anchors.centerIn: parent
        modal: true
        title: qsTr("Clear credentials?")
        standardButtons: Dialog.Yes | Dialog.No

        Label {
            text: qsTr("Remove stored Neurosity credentials from this computer?")
            wrapMode: Text.WordWrap
            width: 320
        }

        onAccepted: {
            if (root.bridge.clearAuth())
                root.load()
        }
    }

}
