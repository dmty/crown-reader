import QtQuick
import QtQuick.Controls
import QtQuick.Layouts
import com.crownreader.app

ApplicationWindow {
    id: window

    width: 1280
    height: 760
    minimumWidth: 720
    minimumHeight: 560
    visible: true
    title: "Crown Reader"
    color: colors.deepField

    property bool showingSettings: false

    AppPalette { id: colors }

    palette.window: colors.deepField
    palette.windowText: colors.readout
    palette.base: colors.panelRaised
    palette.alternateBase: colors.panel
    palette.text: colors.readout
    palette.button: colors.panelRaised
    palette.buttonText: colors.readout
    palette.highlight: colors.cyan
    palette.highlightedText: colors.deepField
    palette.placeholderText: colors.quiet

    function openSettings() {
        if (showingSettings)
            return
        crown.reloadAuthSummary()
        settings.load()
        showingSettings = true
    }

    CrownBridge { id: crown }

    Action {
        id: settingsAction
        text: qsTr("Settings…")
        onTriggered: window.openSettings()
    }

    Shortcut {
        sequences: [StandardKey.Preferences]
        context: Qt.ApplicationShortcut
        onActivated: settingsAction.trigger()
    }

    menuBar: MenuBar {
        Menu {
            title: qsTr("Crown Reader")
            MenuItem { action: settingsAction }
        }
    }

    Timer {
        interval: 33
        running: true
        repeat: true
        onTriggered: crown.tick(Math.round(dashboard.waveformWidth))
    }

    StackLayout {
        anchors.fill: parent
        currentIndex: window.showingSettings ? 1 : 0

        Dashboard {
            id: dashboard
            bridge: crown
            onSettingsRequested: settingsAction.trigger()
        }

        Settings {
            id: settings
            bridge: crown
            onSaved: window.showingSettings = false
            onCancelled: window.showingSettings = false
        }
    }

    Component.onCompleted: {
        crown.reloadAuthSummary()
        if (!crown.configured) {
            settings.load()
            showingSettings = true
        }
    }
}
