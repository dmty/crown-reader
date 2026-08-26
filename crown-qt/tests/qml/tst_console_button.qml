import QtQuick
import QtTest

TestCase {
    id: testCase
    name: "ConsoleButton"
    width: 320
    height: 120
    when: windowShown

    function test_keyboard_focus_has_a_visible_ring() {
        const component = Qt.createComponent("../../qml/ConsoleButton.qml")
        compare(component.status, Component.Ready, component.errorString())
        const button = component.createObject(testCase, {
            text: "CONNECT",
            width: 120,
            height: 40
        })

        verify(button !== null, component.errorString())
        button.forceActiveFocus(Qt.TabFocusReason)
        tryCompare(button, "visualFocus", true)
        compare(button.background.border.width, 2)

        button.destroy()
    }
}
