import QtQuick
import QtQuick.Controls
import com.crownreader.app

ApplicationWindow {
    width: 900
    height: 600
    visible: true
    title: "Crown Reader"

    CrownBridge {
        id: crown
    }

    Timer {
        interval: 33
        running: true
        repeat: true
        onTriggered: crown.tick(content.width)
    }

    // Content can outgrow the window: at MAX_CHANNELS the waveform rows
    // alone exceed a 600px window, which used to push the Connect/Record/Raw
    // buttons off-screen (Column was anchored centered, so it overflowed
    // both ends). Scrolling keeps every control reachable at any channel
    // count instead of capping one.
    ScrollView {
        anchors.fill: parent
        clip: true

        Column {
            id: content
            // Explicit width, not left to size from children: a Column's
            // implicit width is the widest child's width, and a child below
            // binds its own width to `content.width` — leaving this implicit
            // would close that cycle into a binding loop.
            width: Math.min(parent.width - 40, 900)
            anchors.horizontalCenter: parent.horizontalCenter
            y: 16
            spacing: 12

            Text { text: "Status: " + crown.connection; font.pixelSize: 20 }

            Metrics {
                id: metrics
                bridge: crown
                width: content.width
            }

            Repeater {
                // `crown.rev` must be read here, not left implicit:
                // channels() is an invokable, not a bound property, so
                // without this read the model itself would freeze at
                // whatever channels() returned on first evaluation (likely
                // none, before the first successful tick) and never grow.
                model: {
                    crown.rev;
                    return crown.channels();
                }
                Waveform {
                    required property int index
                    width: content.width
                    bridge: crown
                    channel: index
                }
            }

            Row {
                spacing: 8

                Button {
                    text: "Connect"
                    onClicked: crown.start()
                }

                Button {
                    text: crown.recording === "" ? "Record" : "Stop"
                    onClicked: crown.toggleRecording()
                }

                Button {
                    text: crown.raw ? "Raw (next session): on" : "Raw (next session): off"
                    // The choice only takes effect on the session `start()`
                    // spawns next: `supervise` reads it once, at spawn time, and
                    // never again, so flipping it while a session is running
                    // would silently do nothing to that session. Disabled here
                    // rather than left clickable-but-inert.
                    //
                    // A blacklist of the actively-running states, not a
                    // whitelist of "Disconnected"/"Failed": `start()` can also
                    // leave `connection` holding a raw error string (e.g. a
                    // missing-env-var message) on a path that never touches
                    // `Live`, so a whitelist would strand the toggle disabled
                    // forever after a credential error. Anything not in this
                    // list — including that error string — means no session is
                    // running.
                    enabled: ["Scanning", "Connecting", "Authenticating", "Streaming", "Reconnecting"]
                        .indexOf(crown.connection) === -1
                    onClicked: crown.toggleRaw()
                }
            }

            Text {
                text: crown.recording === "" ? "" : "Recording to " + crown.recording
                font.pixelSize: 11
                opacity: 0.7
            }
        }
    }
}
