#ifndef IPCCLIENT_H
#define IPCCLIENT_H

#include <QObject>
#include <QLocalSocket>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>

class IpcClient : public QObject
{
    Q_OBJECT

public:
    enum class PendingRequest {
        None,
        List,
        Search,
        Gallery,
        Delete,
        DeleteAllExceptStarred,
        GetSettings
    };

    explicit IpcClient(QObject *parent = nullptr);
    ~IpcClient();

    Q_INVOKABLE void connectToDaemon();
    Q_INVOKABLE void list(int limit = 50, bool starredOnly = false);
    Q_INVOKABLE void search(const QString &query, int limit = 50);
    Q_INVOKABLE void gallery(int limit = 50);
    Q_INVOKABLE void star(qint64 id, bool value);
    Q_INVOKABLE void copy(qint64 id);
    Q_INVOKABLE void deleteAllExceptStarred();
    Q_INVOKABLE void getSettings();
    Q_INVOKABLE void deleteMultiple(const QList<qint64> &ids);
    Q_INVOKABLE void deleteMultiple(const QVariantList &ids);

signals:
    void connected();
    void disconnected();
    void error(const QString &message);
    void listResponse(const QJsonArray &items);
    void searchResponse(const QJsonArray &items);
    void galleryResponse(const QJsonArray &items);
    void starResponse(bool success);
    void copyResponse(bool success);
    void deleteResponse(qint64 deletedCount);
    void deleteAllExceptStarredResponse(qint64 deletedItems, qint64 deletedImages);
    void settingsReceived(const QJsonObject &settings);
    void requestClose();

private slots:
    void onConnected();
    void onDisconnected();
    void onReadyRead();
    void onError(QLocalSocket::LocalSocketError socketError);

private:
    void sendRequest(const QJsonObject &request);
    QString socketPath() const;

    QLocalSocket *m_socket;
    QString m_buffer;
    PendingRequest m_pending = PendingRequest::None;
};

#endif // IPCCLIENT_H
