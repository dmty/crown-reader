import QtQuick
import QtQuick.Controls

Page {
    id: root

    required property var bridge
    signal saved()
    signal cancelled()

    function selectStoredDevice() {
        var wanted = bridge.deviceid
        for (var i = 0; i < deviceBox.count; i++) {
            if (root.bridge.deviceIdAt(i) === wanted) {
                deviceBox.currentIndex = i
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

    header: Label {
        text: qsTr("Settings")
        font.pixelSize: 20
        padding: 16
    }

    Column {
        width: Math.min(parent.width - 40, 900)
        anchors.horizontalCenter: parent.horizontalCenter
        y: 16
        spacing: 12

        Label { text: qsTr("Email") }
        TextField {
            id: emailField
            width: parent.width
            Accessible.name: qsTr("Email")
        }

        Label { text: qsTr("Password") }
        TextField {
            id: passwordField
            width: parent.width
            echoMode: TextInput.Password
            Accessible.name: qsTr("Password")
        }

        Label { text: qsTr("Device") }
        ComboBox {
            id: deviceBox
            width: parent.width
            model: root.bridge.devicelabels
            property var ids: root.bridge.deviceids
            enabled: count > 0
            Accessible.name: qsTr("Device")
        }

        Button {
            text: qsTr("Load devices")
            onClicked: {
                if (root.bridge.listDevices(emailField.text, passwordField.text))
                    selectStoredDevice()
            }
        }

        Text {
            text: root.bridge.settingserror
            visible: root.bridge.settingserror !== ""
            color: "#c04a4a"
            font.pixelSize: 12
            wrapMode: Text.WordWrap
            width: parent.width
        }

        Row {
            spacing: 8

            Button {
                text: qsTr("Save")
                onClicked: {
                    var deviceId = root.bridge.deviceIdAt(deviceBox.currentIndex)
                    if (root.bridge.savePasswordAuth(emailField.text, passwordField.text, deviceId)) {
                        passwordField.text = ""
                        root.saved()
                    }
                }
            }

            Button {
                text: qsTr("Cancel")
                onClicked: {
                    passwordField.text = ""
                    root.cancelled()
                }
            }

            Button {
                text: qsTr("Clear")
                onClicked: clearDialog.open()
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
