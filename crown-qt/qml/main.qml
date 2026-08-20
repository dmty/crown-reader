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

        Button {
            text: "Connect"
            onClicked: crown.start()
        }
    }
}
