use cxx_qt_build::{CxxQtBuilder, QmlModule};

fn main() {
    CxxQtBuilder::new_qml_module(
        QmlModule::new("com.crownreader.app")
            .qml_file("qml/main.qml")
            .qml_file("qml/Metrics.qml")
            .qml_file("qml/Waveform.qml")
            .qml_file("qml/Settings.qml"),
    )
    // Qt Qml requires linking Qt Network on macOS.
    .qt_module("Network")
    .files(["src/bridge.rs"])
    .build();
}
