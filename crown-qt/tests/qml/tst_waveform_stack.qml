import QtQuick
import QtQuick.Shapes
import QtTest

TestCase {
    id: testCase
    name: "WaveformStack"
    width: 900
    height: 480
    when: windowShown

    Component {
        id: bridgeFixture

        QtObject {
            property int rev: 1
            property bool active: true
            property bool raw: true
            property var channelList: ["CP3", "C3", "F5", "PO3"]

            function channels() {
                return channelList
            }

            function waveform(channel, height) {
                return [Qt.point(0, height / 2), Qt.point(320, height / 2)]
            }
        }
    }

    function findShape(item) {
        if (item instanceof Shape)
            return item
        for (let index = 0; index < item.children.length; index++) {
            const match = findShape(item.children[index])
            if (match)
                return match
        }
        return null
    }

    function findFlickable(item) {
        if (item instanceof Flickable)
            return item
        for (let index = 0; index < item.children.length; index++) {
            const match = findFlickable(item.children[index])
            if (match)
                return match
        }
        return null
    }

    function test_renders_every_channel_in_one_stack() {
        const component = Qt.createComponent("../../qml/WaveformStack.qml")
        compare(component.status, Component.Ready, component.errorString())

        const bridge = bridgeFixture.createObject(testCase)
        const stack = component.createObject(testCase, {
            bridge: bridge,
            width: 780,
            height: 360
        })

        verify(stack !== null, component.errorString())
        tryCompare(stack, "channelCount", 4)
        verify(stack.rowHeight >= 48, "Every trace should retain a readable row height")
        const traceShape = findShape(stack)
        verify(traceShape !== null)
        compare(stack.plotWidth, traceShape.width)

        stack.destroy()
        bridge.destroy()
    }

    function test_revision_reaches_all_supported_channels_through_scrolling() {
        const component = Qt.createComponent("../../qml/WaveformStack.qml")
        compare(component.status, Component.Ready, component.errorString())
        const bridge = bridgeFixture.createObject(testCase)
        const stack = component.createObject(testCase, {
            bridge: bridge,
            width: 780,
            height: 360
        })

        const channels = []
        for (let index = 0; index < 64; index++)
            channels.push("CH" + index)
        bridge.channelList = channels
        bridge.rev++

        tryCompare(stack, "channelCount", 64)
        compare(stack.rowHeight, 48)
        const viewport = findFlickable(stack)
        verify(viewport !== null)
        verify(viewport.contentHeight > viewport.height)

        stack.destroy()
        bridge.destroy()
    }
}
