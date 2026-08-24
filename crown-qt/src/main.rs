pub mod bridge;

use cxx_qt_lib::{QGuiApplication, QQmlApplicationEngine, QUrl};

fn main() {
    // Must run before the QML engine starts: the bridge reads credentials from
    // the environment when Connect is clicked, so the file has to be loaded
    // into the process first. Absent file is the normal case, not an error —
    // already-exported variables take precedence.
    let _ = dotenvy::dotenv();

    let mut app = QGuiApplication::new();
    let mut engine = QQmlApplicationEngine::new();
    if let Some(engine) = engine.as_mut() {
        engine.load(&QUrl::from("qrc:/qt/qml/com/crownreader/app/qml/main.qml"));
    }
    if let Some(app) = app.as_mut() {
        app.exec();
    }
}
