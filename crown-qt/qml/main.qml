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
        onTriggered: {
            if (crown.tick(content.width)) {
                metrics.rev++;
            }
        }
    }

    Column {
        id: content
        anchors.centerIn: parent
        spacing: 12

        Text { text: "Status: " + crown.connection; font.pixelSize: 20 }

        Metrics {
            id: metrics
            bridge: crown
        }

        Repeater {
            // `metrics.rev` must be read here, not just passed to the
            // delegate below: channels() is an invokable, not a bound
            // property, so without this read the model itself would freeze
            // at whatever channels() returned on first evaluation (likely
            // none, before the first successful tick) and never grow.
            model: (metrics.rev, crown.channels())
            Waveform {
                required property int index
                width: content.width
                bridge: crown
                channel: index
                rev: metrics.rev
            }
        }

        Button {
            text: "Connect"
            onClicked: crown.start()
        }
    }
}
