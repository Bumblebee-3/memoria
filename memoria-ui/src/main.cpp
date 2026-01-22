#include <QGuiApplication>
#include <QQmlApplicationEngine>
#include <QQmlContext>
#include "ipcclient.h"

int main(int argc, char *argv[])
{
    QGuiApplication app(argc, argv);
    
    app.setOrganizationName("memoria");
    app.setOrganizationDomain("memoria.local");
    app.setApplicationName("memoria UI");

    QQmlApplicationEngine engine;
    
    IpcClient ipcClient;
    engine.rootContext()->setContextProperty("ipcClient", &ipcClient);
    
    const QUrl url(QStringLiteral("qrc:/qml/main.qml"));
    QObject::connect(&engine, &QQmlApplicationEngine::objectCreated,
                     &app, [url](QObject *obj, const QUrl &objUrl) {
        if (!obj && url == objUrl)
            QCoreApplication::exit(-1);
    }, Qt::QueuedConnection);
    
    engine.load(url);
    
    ipcClient.connectToDaemon();

    return app.exec();
}
