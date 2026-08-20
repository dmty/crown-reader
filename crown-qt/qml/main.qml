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

            // `crown.error` only ever carries a session-ending failure
            // (`supervise` returns on a terminal error, never a transient
            // one) so this is the one place a windowed user sees a wrong
            // password or a missing credential — the message is otherwise
            // only on stderr, which a Finder-launched app has no way to show.
            Text {
                text: crown.error === "" ? "" : "Error: " + crown.error
                visible: crown.error !== ""
                color: "#c04a4a"
                font.pixelSize: 12
                wrapMode: Text.WordWrap
                width: content.width
            }

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
                    // A device must be configured before there's anything
                    // to record against — `toggleRecording()` no-ops with
                    // only a stderr line otherwise, which an app launched
                    // outside a terminal (e.g. from Finder) never sees.
                    // Once true this never goes false again (`Live.device`
                    // is only ever replaced, never cleared), so this never
                    // fights with an in-progress recording's "Stop" state.
                    enabled: crown.ready
                    onClicked: crown.toggleRecording()
                }

                Button {
                    // Two different facts share this one property, by
                    // session phase: idle, `crown.raw` is the pending choice
                    // the next `start()` will use, and the label says so;
                    // active, `tick()` overwrites it with what the transport
                    // actually did (see bridge.rs), so the same property
                    // reads as current status instead — including going
                    // "off" for a few seconds while a session that will
                    // succeed is still scanning/connecting/authenticating,
                    // before the raw subscribe has had a chance to land.
                    text: crown.active
                        ? (crown.raw ? "Raw: on" : "Raw: off")
                        : (crown.raw ? "Raw (next session): on" : "Raw (next session): off")
                    // The choice only takes effect on the session `start()`
                    // spawns next: `supervise` reads it once, at spawn time, and
                    // never again, so flipping it while a session is running
                    // would silently do nothing to that session. Disabled here
                    // rather than left clickable-but-inert.
                    //
                    // `crown.active` is `ConnectionState::is_active()`
                    // republished, not a string match against `connection`:
                    // matching against `label()`'s output here would silently
                    // break if that mapping ever changed, with no compiler
                    // error to catch it.
                    enabled: !crown.active
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
