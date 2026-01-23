#include "ipcclient.h"
#include <QStandardPaths>
#include <QDir>
#include <QDebug>
#include <unistd.h>

IpcClient::IpcClient(QObject *parent)
    : QObject(parent)
    , m_socket(new QLocalSocket(this))
{
    connect(m_socket, &QLocalSocket::connected, this, &IpcClient::onConnected);
    connect(m_socket, &QLocalSocket::disconnected, this, &IpcClient::onDisconnected);
    connect(m_socket, &QLocalSocket::readyRead, this, &IpcClient::onReadyRead);
    connect(m_socket, &QLocalSocket::errorOccurred, this, &IpcClient::onError);
}

IpcClient::~IpcClient()
{
    if (m_socket->state() == QLocalSocket::ConnectedState) {
        m_socket->disconnectFromServer();
    }
}

QString IpcClient::socketPath() const
{
    QString runtimeDir = qEnvironmentVariable("XDG_RUNTIME_DIR");
    if (runtimeDir.isEmpty()) {
        runtimeDir = QString("/run/user/%1").arg(getuid());
    }
    return QDir(runtimeDir).filePath("memoria.sock");
}

void IpcClient::connectToDaemon()
{
    QString path = socketPath();
    qDebug() << "Connecting to daemon at:" << path;
    m_socket->connectToServer(path);
}

void IpcClient::sendRequest(const QJsonObject &request)
{
    if (m_socket->state() != QLocalSocket::ConnectedState) {
        emit error("Not connected to daemon. Is memoria-daemon running?");
        return;
    }

    QJsonDocument doc(request);
    QByteArray data = doc.toJson(QJsonDocument::Compact) + "\n";
    
    qint64 written = m_socket->write(data);
    if (written == -1) {
        emit error("Failed to send request to daemon");
        return;
    }
    
    m_socket->flush();
}

void IpcClient::list(int limit, bool starredOnly)
{
    m_pending = PendingRequest::List;
    QJsonObject req;
    req["cmd"] = "list";
    QJsonObject args;
    args["limit"] = limit;
    args["starred_only"] = starredOnly;
    req["args"] = args;
    sendRequest(req);
}

void IpcClient::search(const QString &query, int limit)
{
    m_pending = PendingRequest::Search;
    QJsonObject req;
    req["cmd"] = "search";
    QJsonObject args;
    args["query"] = query;
    args["limit"] = limit;
    req["args"] = args;
    sendRequest(req);
}

void IpcClient::gallery(int limit)
{
    m_pending = PendingRequest::Gallery;
    QJsonObject req;
    req["cmd"] = "gallery";
    QJsonObject args;
    args["limit"] = limit;
    req["args"] = args;
    sendRequest(req);
}

void IpcClient::star(qint64 id, bool value)
{
    QJsonObject req;
    req["cmd"] = "star";
    QJsonObject args;
    args["id"] = id;
    args["value"] = value;
    req["args"] = args;
    sendRequest(req);
}

void IpcClient::copy(qint64 id)
{
    QJsonObject req;
    req["cmd"] = "copy";
    QJsonObject args;
    args["id"] = id;
    req["args"] = args;
    sendRequest(req);
}


void IpcClient::deleteAllExceptStarred()
{
    m_pending = PendingRequest::DeleteAllExceptStarred;
    QJsonObject req;
    req["cmd"] = "delete_all_except_starred";
    sendRequest(req);
}

void IpcClient::deleteMultiple(const QList<qint64> &ids)
{
    QJsonObject req;
    req["cmd"] = "delete_items";
    QJsonObject args;
    QJsonArray arr;
    for (qint64 id : ids) {
        arr.append(static_cast<double>(id));
    }
    args["ids"] = arr;
    req["args"] = args;
    sendRequest(req);
}

void IpcClient::deleteMultiple(const QVariantList &ids)
{
    QList<qint64> list;
    list.reserve(ids.size());
    for (const QVariant &v : ids) {
        list.append(v.toLongLong());
    }
    deleteMultiple(list);
}

void IpcClient::getSettings()
{
    m_pending = PendingRequest::GetSettings;
    QJsonObject req;
    req["cmd"] = "get_settings";
    sendRequest(req);
}

void IpcClient::onConnected()
{
    qDebug() << "Connected to daemon";
    emit connected();
}

void IpcClient::onDisconnected()
{
    qDebug() << "Disconnected from daemon";
    emit disconnected();
}

void IpcClient::onReadyRead()
{
    m_buffer += QString::fromUtf8(m_socket->readAll());
    
    int newlinePos;
    while ((newlinePos = m_buffer.indexOf('\n')) != -1) {
        QString line = m_buffer.left(newlinePos).trimmed();
        m_buffer = m_buffer.mid(newlinePos + 1);
        
        if (line.isEmpty()) {
            continue;
        }
        
        QJsonDocument doc = QJsonDocument::fromJson(line.toUtf8());
        if (doc.isNull() || !doc.isObject()) {
            qWarning() << "Invalid JSON response:" << line;
            emit error("Received malformed response from daemon");
            continue;
        }
        
        QJsonObject response = doc.object();
        bool ok = response["ok"].toBool();
        
        if (!ok) {
            QString errorMsg = response["error"].toString("Unknown daemon error");
            qWarning() << "Daemon error:" << errorMsg;
            emit error(errorMsg);
            continue;
        }
        
        QJsonValue dataVal = response["data"];
        
        if (dataVal.isArray()) {
            QJsonArray items = dataVal.toArray();
            
            for (int i = 0; i < items.size(); ++i) {
                QJsonObject item = items[i].toObject();
                if (item.contains("id")) {
                    item["itemId"] = item["id"];
                    item.remove("id");
                    items[i] = item;
                }
            }
            
            switch (m_pending) {
            case PendingRequest::List:
                emit listResponse(items);
                break;
            case PendingRequest::Search:
                emit searchResponse(items);
                break;
            case PendingRequest::Gallery:
                emit galleryResponse(items);
                break;
            default:
                break;
            }
            
            m_pending = PendingRequest::None;
        } else if (dataVal.isObject()) {
            QJsonObject dataObj = dataVal.toObject();
            
            if (dataObj.contains("updated")) {
                emit starResponse(true);
            } else if (dataObj.contains("copied")) {
                emit copyResponse(true);
                emit requestClose();
            } else if (dataObj.contains("deleted")) {
                qint64 deletedCount = static_cast<qint64>(dataObj.value("deleted").toDouble(0));
                emit deleteResponse(deletedCount);
                m_pending = PendingRequest::None;
            } else if (dataObj.contains("deleted_items") || dataObj.contains("deleted_images")) {
                qint64 deletedItems = static_cast<qint64>(dataObj.value("deleted_items").toDouble(0));
                qint64 deletedImages = static_cast<qint64>(dataObj.value("deleted_images").toDouble(0));
                emit deleteAllExceptStarredResponse(deletedItems, deletedImages);
            } else if (dataObj.contains("deleted_count")) {
                qint64 deletedCount = static_cast<qint64>(dataObj.value("deleted_count").toDouble(0));
                emit deleteResponse(deletedCount);
            } else if (dataObj.contains("ui") || dataObj.contains("grid")) {
                emit settingsReceived(dataObj);
            }
        }
    }
}

void IpcClient::onError(QLocalSocket::LocalSocketError socketError)
{
    QString errorMsg;
    switch (socketError) {
    case QLocalSocket::ServerNotFoundError:
        errorMsg = "Daemon socket not found. Start memoria-daemon first.";
        break;
    case QLocalSocket::ConnectionRefusedError:
        errorMsg = "Connection refused. Is memoria-daemon running?";
        break;
    case QLocalSocket::SocketAccessError:
        errorMsg = "Permission denied accessing daemon socket";
        break;
    case QLocalSocket::SocketResourceError:
        errorMsg = "System resource error communicating with daemon";
        break;
    case QLocalSocket::SocketTimeoutError:
        errorMsg = "Daemon connection timeout";
        break;
    default:
        errorMsg = QString("Socket error: %1").arg(m_socket->errorString());
        break;
    }
    
    qWarning() << "Socket error:" << socketError << "-" << errorMsg;
    emit error(errorMsg);
}
