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

    Column {
        id: content
        anchors.centerIn: parent
        spacing: 12

        Text { text: "Status: " + crown.connection; font.pixelSize: 20 }
        Text { text: "Calm: " + crown.calm.toFixed(2) }
        Text { text: "Focus: " + crown.focus.toFixed(2) }
        Text { text: "Dropped: " + crown.dropped }

        Button {
            text: "Connect"
            onClicked: crown.start()
        }
    }
}
