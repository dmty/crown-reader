import QtQuick
import QtQuick.Controls.Basic
import QtQuick.Layouts
import QtTest

TestCase {
    id: testCase
    name: "Settings"
    width: 900
    height: 700
    when: windowShown

    Component {
        id: bridgeFixture

        QtObject {
            property string email: "reader@example.com"
            property string deviceid: "device-83c"
            property string settingserror: ""
            property var devicelabels: ["Crown-83C"]

            function deviceIdAt(index) { return index === 0 ? "device-83c" : "" }
            function listDevices(email, password) { return true }
            function savePasswordAuth(email, password, deviceId) { return true }
            function clearAuth() { return true }
        }
    }

    Component {
        id: windowFixture

        ApplicationWindow {
            property alias stack: productionStack
            width: 900
            height: 700
            visible: false

            StackLayout {
                id: productionStack
                anchors.fill: parent
            }
        }
    }

    function findText(item, value) {
        if (item.text === value)
            return item
        for (let index = 0; index < item.children.length; index++) {
            const match = findText(item.children[index], value)
            if (match)
                return match
        }
        return null
    }

    function findComboBox(item) {
        if (item instanceof ComboBox)
            return item
        for (let index = 0; index < item.children.length; index++) {
            const match = findComboBox(item.children[index])
            if (match)
                return match
        }
        return null
    }

    function findButton(item, text) {
        if (item instanceof Button && item.text === text)
            return item
        for (let index = 0; index < item.children.length; index++) {
            const match = findButton(item.children[index], text)
            if (match)
                return match
        }
        return null
    }

    function findPasswordField(item) {
        if (item instanceof TextField && item.echoMode === TextInput.Password)
            return item
        for (let index = 0; index < item.children.length; index++) {
            const match = findPasswordField(item.children[index])
            if (match)
                return match
        }
        return null
    }

    function test_account_heading_precedes_its_description() {
        const component = Qt.createComponent("../../qml/Settings.qml")
        compare(component.status, Component.Ready, component.errorString())
        const bridge = bridgeFixture.createObject(testCase)
        const settings = component.createObject(testCase, {
            bridge: bridge,
            width: 900,
            height: 700
        })

        verify(settings !== null, component.errorString())
        wait(0)
        const heading = findText(settings, "Connect your account")
        const description = findText(settings,
            "Credentials stay in the system keyring. Load your claimed devices, choose the Crown you want to monitor, then save.")
        verify(heading !== null)
        verify(description !== null)

        const headingY = heading.mapToItem(settings, 0, 0).y
        const descriptionY = description.mapToItem(settings, 0, 0).y
        verify(heading.height > 0)
        verify(headingY + heading.height < descriptionY)

        settings.destroy()
        bridge.destroy()
    }

    function test_device_selector_popup_uses_the_claimed_device_model() {
        const component = Qt.createComponent("../../qml/Settings.qml")
        compare(component.status, Component.Ready, component.errorString())
        const bridge = bridgeFixture.createObject(testCase)
        const settings = component.createObject(testCase, {
            bridge: bridge,
            width: 900,
            height: 700
        })

        verify(settings !== null, component.errorString())
        const selector = findComboBox(settings)
        verify(selector !== null)
        compare(selector.count, 1)
        compare(selector.displayText, "Crown-83C")

        selector.popup.open()
        tryCompare(selector.popup, "visible", true)
        tryCompare(selector.popup.contentItem, "count", 1)
        selector.popup.close()

        settings.destroy()
        bridge.destroy()
    }

    function test_close_clears_the_password_field() {
        const component = Qt.createComponent("../../qml/Settings.qml")
        compare(component.status, Component.Ready, component.errorString())
        const bridge = bridgeFixture.createObject(testCase)
        const settings = component.createObject(testCase, {
            bridge: bridge,
            width: 900,
            height: 700
        })

        const password = findPasswordField(settings)
        const close = findButton(settings, "CLOSE")
        verify(password !== null)
        verify(close !== null)
        password.text = "secret-value"
        close.clicked()
        compare(password.text, "")

        settings.destroy()
        bridge.destroy()
    }

    function test_account_heading_has_one_content_inset_inside_the_application_window() {
        const component = Qt.createComponent("../../qml/Settings.qml")
        compare(component.status, Component.Ready, component.errorString())
        const bridge = bridgeFixture.createObject(testCase)
        const window = windowFixture.createObject(testCase)
        const settings = component.createObject(window.stack, { bridge: bridge })

        verify(settings !== null, component.errorString())
        wait(0)
        const heading = findText(settings, "Connect your account")
        verify(heading !== null)
        const headingY = heading.mapToItem(window.contentItem, 0, 0).y
        compare(headingY, settings.header.height + 30)

        settings.destroy()
        window.destroy()
        bridge.destroy()
    }
}
