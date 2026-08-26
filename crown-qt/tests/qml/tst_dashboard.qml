import QtQuick
import QtTest

TestCase {
    id: testCase
    name: "Dashboard"
    width: 1360
    height: 820
    when: windowShown

    Component {
        id: bridgeFixture

        QtObject {
            property string connection: "Streaming"
            property real calm: 0.72
            property real focus: 0.48
            property int dropped: 0
            property int staleness: 240
            property string recording: ""
            property bool raw: true
            property bool active: true
            property bool ready: true
            property bool configured: true
            property string error: ""
            property string deviceid: "Crown-83C"
            property int rev: 1

            function channels() {
                return ["CP3", "C3", "F5", "PO3"]
            }

            function waveform(channel, height) {
                return [Qt.point(0, height / 2), Qt.point(320, height / 2)]
            }

            function quality(channel) {
                return channel === 3 ? "Good" : "Great"
            }

            function band(name) {
                return 0.125
            }

            function start() {}
            function toggleRecording() {}
            function toggleRaw() {}
        }
    }

    function createDashboard(width, height) {
        const component = Qt.createComponent("../../qml/Dashboard.qml")
        compare(component.status, Component.Ready, component.errorString())
        const bridge = bridgeFixture.createObject(testCase)
        const dashboard = component.createObject(testCase, {
            bridge: bridge,
            width: width,
            height: height
        })
        verify(dashboard !== null, component.errorString())
        return { dashboard: dashboard, bridge: bridge }
    }

    function findTextContaining(item, value) {
        if (typeof item.text === "string" && item.text.indexOf(value) !== -1)
            return item
        for (let index = 0; index < item.children.length; index++) {
            const match = findTextContaining(item.children[index], value)
            if (match)
                return match
        }
        return null
    }

    function test_waveforms_are_center_stage_on_desktop() {
        const fixture = createDashboard(1360, 820)
        const signal = findChild(fixture.dashboard, "signalPanel")
        const waveform = findChild(fixture.dashboard, "waveformPanel")
        const metrics = findChild(fixture.dashboard, "metricsPanel")

        verify(signal !== null)
        verify(waveform !== null)
        verify(metrics !== null)
        verify(waveform.width > signal.width)
        verify(waveform.width > metrics.width)
        verify(signal.mapToItem(fixture.dashboard, 0, 0).x
               < waveform.mapToItem(fixture.dashboard, 0, 0).x)
        verify(waveform.mapToItem(fixture.dashboard, 0, 0).x
               < metrics.mapToItem(fixture.dashboard, 0, 0).x)

        fixture.dashboard.destroy()
        fixture.bridge.destroy()
    }

    function test_waveforms_move_first_when_compact() {
        const fixture = createDashboard(760, 900)
        const signal = findChild(fixture.dashboard, "signalPanel")
        const waveform = findChild(fixture.dashboard, "waveformPanel")

        compare(fixture.dashboard.compact, true)
        verify(waveform.mapToItem(fixture.dashboard, 0, 0).y
               < signal.mapToItem(fixture.dashboard, 0, 0).y)

        fixture.dashboard.destroy()
        fixture.bridge.destroy()
    }

    function test_terminal_error_is_not_truncated() {
        const fixture = createDashboard(1000, 720)
        fixture.bridge.error = "Authentication failed: "
            + "the identity provider rejected the cached credential; ".repeat(16)
        wait(0)

        const errorText = findTextContaining(fixture.dashboard, "STREAM ERROR")
        verify(errorText !== null)
        verify(!errorText.truncated, "The complete terminal error must remain available in the GUI")

        fixture.dashboard.destroy()
        fixture.bridge.destroy()
    }

    function test_cached_metrics_are_not_live_while_disconnected() {
        const fixture = createDashboard(1000, 720)
        fixture.bridge.connection = "Disconnected"
        fixture.bridge.active = false
        wait(0)

        const offline = findTextContaining(fixture.dashboard, "OFFLINE")
        verify(offline !== null, "Cached metrics must be labelled offline after the stream stops")

        fixture.dashboard.destroy()
        fixture.bridge.destroy()
    }
}
