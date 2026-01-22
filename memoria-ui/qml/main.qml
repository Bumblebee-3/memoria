import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: root
    visible: true
    width: 800
    height: 600
    title: "memoria"
    color: "#000000"


    property string viewMode: "list"        // list | gallery
    property bool favoritesOnly: false
    property bool initialLoadDone: false
    property var selectedIds: ({})  // Set of selected item IDs for multi-select
    property int selectionAnchor: -1        // For shift+click range selection
    property int uiWidth: 800
    property int uiHeight: 600
    property var gridSettings: ({})         // Grid and UI settings from daemon

    function toggleSelection(clipId) {
        var newSelected = Object.assign({}, selectedIds)
        if (newSelected[clipId]) {
            delete newSelected[clipId]
        } else {
            newSelected[clipId] = true
        }
        selectedIds = newSelected
    }

    function clearSelection() {
        selectedIds = {}
        selectionAnchor = -1
    }

    function isSelected(clipId) {
        return selectedIds[clipId] === true
    }

    function getSelectedCount() {
        return Object.keys(selectedIds).length
    }

    function extendSelectionList(fromIndex, toIndex) {
        var newSelected = Object.assign({}, selectedIds)
        var start = Math.min(fromIndex, toIndex)
        var end = Math.max(fromIndex, toIndex)
        for (var i = start; i <= end; i++) {
            if (i < listModel.count) {
                newSelected[listModel.get(i).clipId] = true
            }
        }
        selectedIds = newSelected
    }

    function extendSelectionGrid(fromIndex, toIndex) {
        var newSelected = Object.assign({}, selectedIds)
        var start = Math.min(fromIndex, toIndex)
        var end = Math.max(fromIndex, toIndex)
        for (var i = start; i <= end; i++) {
            if (i < galleryModel.count) {
                newSelected[galleryModel.get(i).clipId] = true
            }
        }
        selectedIds = newSelected
    }

    function deleteSelected() {
        var ids = Object.keys(selectedIds).map(function(id) { return parseInt(id) })
        if (ids.length > 0) {
            statusBar.text = "Deleting " + ids.length + " item" + (ids.length > 1 ? "s" : "") + "…"
            statusBar.color = "#9ca3af"
            ipcClient.deleteMultiple(ids)
        } else {
            // Fallback: delete current focused item
            var currentId = -1
            if (viewMode === "list" && contentList.currentIndex >= 0 && contentList.currentIndex < listModel.count) {
                currentId = listModel.get(contentList.currentIndex).clipId
            } else if (viewMode === "gallery" && contentGrid.currentIndex >= 0 && contentGrid.currentIndex < galleryModel.count) {
                currentId = galleryModel.get(contentGrid.currentIndex).clipId
            }
            if (currentId > 0) {
                statusBar.text = "Deleting 1 item…"
                statusBar.color = "#9ca3af"
                var forced = {}
                forced[currentId] = true
                selectedIds = forced
                ipcClient.deleteMultiple([ currentId ])
            }
        }
    }


    Connections {
        target: ipcClient

        function onConnected() {
            statusBar.text = "Connected"
            statusBar.color = "#4ade80"

            if (!initialLoadDone) {
                ipcClient.getSettings()
                ipcClient.list(100)
                initialLoadDone = true
            }
        }

        function onSettingsReceived(settings) {
            gridSettings = settings
            if (settings.ui && settings.ui.width) {
                uiWidth = settings.ui.width
                uiHeight = settings.ui.height
                root.width = uiWidth
                root.height = uiHeight
            }
            if (settings.ui && settings.ui.opacity) {
                root.opacity = settings.ui.opacity
            }
        }

        function onDisconnected() {
            statusBar.text = "Disconnected"
            statusBar.color = "#ef4444"
        }

        function onError(msg) {
            statusBar.text = "Error: " + msg
            statusBar.color = "#ef4444"
        }

        function onListResponse(items) {
            listModel.clear()
            clearSelection()

            for (let i = 0; i < items.length; i++) {
                listModel.append({
                    clipId: items[i].itemId,
                    title: items[i].title || "",
                    body: items[i].body || "",
                    starred: items[i].starred === true,
                    hasImage: items[i].has_image === true,
                    thumbnailPath: items[i].thumbnail_path || "",
                    selected: false
                })
            }

            if (listModel.count > 0) {
                contentList.currentIndex = 0
            }
        }

        function onSearchResponse(items) {
            clearSelection()
            onListResponse(items)
        }
        function onGalleryResponse(items) {
            galleryModel.clear()
            clearSelection()

            for (let i = 0; i < items.length; i++) {
                galleryModel.append({
                    clipId: items[i].itemId,
                    title: items[i].title || "",
                    starred: items[i].starred === true,
                    hasImage: items[i].has_image === true,
                    thumbnailPath: items[i].thumbnail_path || "",
                    selected: false
                })
            }

            if (galleryModel.count > 0) {
                contentGrid.currentIndex = 0
            }
        }

        function onCopyResponse(ok) {
            if (ok) {
                statusBar.text = "Copied"
                statusBar.color = "#4ade80"
                Qt.callLater(Qt.quit)
            }
        }

        function onDeleteResponse(deletedCount) {
            if (deletedCount > 0) {
                statusBar.text = "Deleted " + deletedCount + " item" + (deletedCount > 1 ? "s" : "")
                statusBar.color = "#4ade80"
                
                // Remove deleted items from the current model
                var deletedIds = Object.keys(selectedIds)
                for (let i = 0; i < deletedIds.length; i++) {
                    var id = parseInt(deletedIds[i])
                    // Remove from list model
                    for (let j = 0; j < listModel.count; j++) {
                        if (listModel.get(j).clipId === id) {
                            listModel.remove(j)
                            break
                        }
                    }
                    // Remove from gallery model
                    for (let j = 0; j < galleryModel.count; j++) {
                        if (galleryModel.get(j).clipId === id) {
                            galleryModel.remove(j)
                            break
                        }
                    }
                }
                
                clearSelection()
            } else {
                statusBar.text = "Nothing to delete (all starred)"
                statusBar.color = "#fbbf24"
                clearSelection()
            }
        }

        function onDeleteAllExceptStarredResponse(deletedItems, deletedImages) {
            statusBar.text = "Deleted " + deletedItems + " items"
            statusBar.color = "#4ade80"
            // Refresh current view
            if (viewMode === "gallery") {
                ipcClient.gallery(200)
            } else {
                ipcClient.list(200, favoritesOnly)
            }
        }
    }

    Dialog {
    id: deleteConfirmDialog
    modal: true
    focus: true
    width: 360
    height: 190

    anchors.centerIn: Overlay.overlay

    background: Rectangle {
        radius: 10
        color: "#332f3b"
        border.color: "#6128a3"
        border.width: 3
    }

    contentItem: Rectangle {
        color: "transparent"
        anchors.fill: parent
        anchors.margins: 20   // ✅ THIS replaces padding

        ColumnLayout {
            anchors.fill: parent
            spacing: 16

            // ---- Title ----
            Text {
                text: "Confirm deletion"
                font.pixelSize: 16
                font.bold: true
                color: "#f9fafb"
                Layout.alignment: Qt.AlignHCenter
            }

            Rectangle {
                height: 1
                Layout.fillWidth: true
                color: "#374151"
            }

            // ---- Warning ----
            RowLayout {
                spacing: 12
                Layout.fillWidth: true

                Text {
                    text: "⚠"
                    font.pixelSize: 26
                    color: "#fbbf24"
                    Layout.alignment: Qt.AlignTop
                }

                ColumnLayout {
                    spacing: 6
                    Layout.fillWidth: true

                    Text {
                        text: "This will permanently delete all non-starred clipboard items."
                        wrapMode: Text.WordWrap
                        color: "#e5e7eb"
                        font.pixelSize: 13
                        Layout.fillWidth: true
                    }

                    Text {
                        text: "Starred items will be kept."
                        color: "#9ca3af"
                        font.pixelSize: 12
                    }
                }
            }

            // ---- Buttons ----
            RowLayout {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignRight
                spacing: 10

                Button {
                    text: "Cancel"
                    background: Rectangle {
                        radius: 6
                        color: "#ccbfbf"
                    }

                    onClicked: deleteConfirmDialog.reject()
                }

                Button {
                    text: "Delete"
                    highlighted: true

                    background: Rectangle {
                        radius: 6
                        color: "#dc2626"
                    }

                    onClicked: deleteConfirmDialog.accept()
                }
            }
        }
    }

    onAccepted: {
        statusBar.text = "Deleting…"
        statusBar.color = "#9ca3af"
        ipcClient.deleteAllExceptStarred()
    }
}



    Shortcut { sequence: "Escape"; onActivated: Qt.quit() }

    Shortcut {
        sequence: "/"
        onActivated: {
            searchField.forceActiveFocus()
            searchField.selectAll()
        }
    }

    Shortcut {
        sequence: "Down"
        enabled: searchField.activeFocus && viewMode === "list"
        onActivated: contentList.forceActiveFocus()
    }

    Shortcut {
        sequence: "Down"
        enabled: searchField.activeFocus && viewMode === "gallery"
        onActivated: contentGrid.forceActiveFocus()
    }

    // Shift+Down: extend selection down in list
    Shortcut {
        sequence: "Shift+Down"
        enabled: viewMode === "list" && contentList.activeFocus && contentList.currentIndex >= 0
        onActivated: {
            if (selectionAnchor === -1) {
                selectionAnchor = contentList.currentIndex
            }
            let newIdx = Math.min(contentList.currentIndex + 1, listModel.count - 1)
            extendSelectionList(selectionAnchor, newIdx)
            contentList.currentIndex = newIdx
        }
    }

    // Shift+Up: extend selection up in list
    Shortcut {
        sequence: "Shift+Up"
        enabled: viewMode === "list" && contentList.activeFocus && contentList.currentIndex >= 0
        onActivated: {
            if (selectionAnchor === -1) {
                selectionAnchor = contentList.currentIndex
            }
            let newIdx = Math.max(contentList.currentIndex - 1, 0)
            extendSelectionList(selectionAnchor, newIdx)
            contentList.currentIndex = newIdx
        }
    }

    // Shift+Down: extend selection down in gallery
    Shortcut {
        sequence: "Shift+Down"
        enabled: viewMode === "gallery" && contentGrid.activeFocus && contentGrid.currentIndex >= 0
        onActivated: {
            if (selectionAnchor === -1) {
                selectionAnchor = contentGrid.currentIndex
            }
            let cols = gridSettings.grid ? gridSettings.grid.columns : 3
            let newIdx = Math.min(contentGrid.currentIndex + cols, galleryModel.count - 1)
            extendSelectionGrid(selectionAnchor, newIdx)
            contentGrid.currentIndex = newIdx
        }
    }

    // Shift+Up: extend selection up in gallery
    Shortcut {
        sequence: "Shift+Up"
        enabled: viewMode === "gallery" && contentGrid.activeFocus && contentGrid.currentIndex >= 0
        onActivated: {
            if (selectionAnchor === -1) {
                selectionAnchor = contentGrid.currentIndex
            }
            let cols = gridSettings.grid ? gridSettings.grid.columns : 3
            let newIdx = Math.max(contentGrid.currentIndex - cols, 0)
            extendSelectionGrid(selectionAnchor, newIdx)
            contentGrid.currentIndex = newIdx
        }
    }

    // Delete: delete all selected items
    Shortcut {
        sequence: "Delete"
            enabled: (viewMode === "list" || viewMode === "gallery") && (
                getSelectedCount() > 0 ||
                (viewMode === "list" && contentList.currentIndex >= 0 && contentList.currentIndex < listModel.count) ||
                (viewMode === "gallery" && contentGrid.currentIndex >= 0 && contentGrid.currentIndex < galleryModel.count)
            )
        onActivated: {
            deleteSelected()
        }
    }

    Shortcut {
        sequence: "Return"
        enabled: viewMode === "list" && contentList.activeFocus && contentList.currentIndex >= 0
        onActivated: {
            // If items selected, copy first selected; otherwise copy current
            if (getSelectedCount() > 0) {
                let firstId = parseInt(Object.keys(selectedIds)[0])
                ipcClient.copy(firstId)
            } else {
                ipcClient.copy(listModel.get(contentList.currentIndex).clipId)
            }
        }
    }

    Shortcut {
        sequence: "Return"
        enabled: viewMode === "gallery" && contentGrid.activeFocus && contentGrid.currentIndex >= 0
        onActivated: {
            // If items selected, copy first selected; otherwise copy current
            if (getSelectedCount() > 0) {
                let firstId = parseInt(Object.keys(selectedIds)[0])
                ipcClient.copy(firstId)
            } else {
                ipcClient.copy(galleryModel.get(contentGrid.currentIndex).clipId)
            }
        }
    }


    ColumnLayout {
        anchors.fill: parent
        spacing: 0


        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 56
            color: "#313033"

            RowLayout {
                anchors.fill: parent
                anchors.margins: 10
                spacing: 10

                TextField {
                    id: searchField
                    Layout.fillWidth: true
                    placeholderText: "Search… (/ to focus, Enter to search)"
                    color: "#ffffff"
                    placeholderTextColor: "#FFFFFF"

                    background: Rectangle {
                        radius: 4
                        color: "#3d3d3d"
                        border.color: searchField.activeFocus ? "#654ade" : "#4d4d4d"
                    }

                    Keys.onReturnPressed: {
                        const q = text.trim()

                        viewMode = "list"

                        if (q.length > 0) {
                            ipcClient.search(q, 100)
                        } else {
                            ipcClient.list(100, favoritesOnly)
                        }
                    }
                }


                // ===== List =====
                Button {
                    id: listBtn
                    text: "List"
                    property bool active: viewMode === "list"

                    padding: 0
                    implicitHeight: 26
                    implicitWidth: contentItem.implicitWidth + 16

                    background: Rectangle {
                        radius: 4
                        color: listBtn.active
                            ? "#3a3f4b"
                            : listBtn.hovered
                                ? "#2c303a"
                                : "#1f222a"
                        border.color: "#444"
                        border.width: 1
                    }

                    contentItem: Text {
                        text: listBtn.text
                        color: listBtn.active ? "#ffffff" : "#d0d0d0"
                        font.pixelSize: 13
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }

                    onClicked: {
                        viewMode = "list"
                        favoritesOnly = false
                        searchField.text = ""
                        clearSelection()
                        ipcClient.list(100)
                    }
                }

                // ===== Gallery =====
                Button {
                    id: galleryBtn
                    text: "Gallery"
                    property bool active: viewMode === "gallery"

                    padding: 0
                    implicitHeight: 26
                    implicitWidth: contentItem.implicitWidth + 16

                    background: Rectangle {
                        radius: 4
                        color: galleryBtn.active
                            ? "#3a3f4b"
                            : galleryBtn.hovered
                                ? "#2c303a"
                                : "#1f222a"
                        border.color: "#444"
                        border.width: 1
                    }

                    contentItem: Text {
                        text: galleryBtn.text
                        color: galleryBtn.active ? "#ffffff" : "#d0d0d0"
                        font.pixelSize: 13
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }

                    onClicked: {
                        viewMode = "gallery"
                        favoritesOnly = false
                        searchField.text = ""
                        clearSelection()
                        ipcClient.gallery(200)
                    }
                }

                // ===== Favorites =====
                Button {
                    id: favBtn
                    text: "⭐"
                    property bool active: favoritesOnly

                    padding: 0
                    implicitHeight: 26
                    implicitWidth: 28

                    background: Rectangle {
                        radius: 4
                        color: favBtn.active
                            ? "#3a3f4b"
                            : favBtn.hovered
                                ? "#2c303a"
                                : "#1f222a"
                        border.color: "#444"
                        border.width: 1
                    }

                    contentItem: Text {
                        text: favBtn.text
                        color: favBtn.active ? "#ffd75f" : "#d0d0d0"
                        font.pixelSize: 14
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }

                    onClicked: {
                        viewMode = "list"
                        favoritesOnly = !favoritesOnly
                        searchField.text = ""
                        clearSelection()
                        ipcClient.list(200, favoritesOnly)
                    }
                }

                // ===== Delete =====
                Button {
                    id: deleteBtn
                    text: "🗑"

                    padding: 0
                    implicitHeight: 26
                    implicitWidth: 28

                    background: Rectangle {
                        radius: 4
                        color: deleteBtn.hovered ? "#3a2a2a" : "#1f222a"
                        border.color: "#444"
                        border.width: 1
                    }

                    contentItem: Text {
                        text: deleteBtn.text
                        color: "#d0d0d0"
                        font.pixelSize: 14
                        horizontalAlignment: Text.AlignHCenter
                        verticalAlignment: Text.AlignVCenter
                    }

                    onClicked: deleteConfirmDialog.open()

                    ToolTip.visible: hovered
                    ToolTip.text: "Delete all non-starred"
                }

            }
        }


        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: viewMode === "gallery" ? 1 : 0


            ListView {
                id: contentList
                focus: false
                clip: true
                spacing: 1
                model: ListModel { id: listModel }

                delegate: Rectangle {
                    width: contentList.width
                    height: Math.max(80, contentColumn.implicitHeight + 20)
                    color: isSelected(model.clipId) ? "#424242" : (contentList.currentIndex === index ? "#424242" : "#1e1e1e")
                    border.color: isSelected(model.clipId) ? "#974ade" : (contentList.currentIndex === index ? "#974ade" : "transparent")
                    border.width: 2

                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            contentList.currentIndex = index
                            if (!(mouse.modifiers & Qt.ShiftModifier)) {
                                clearSelection()
                                selectionAnchor = -1
                            }
                            ipcClient.copy(clipId)
                        }
                    }

                    RowLayout {
                        id: row
                        anchors.fill: parent
                        anchors.margins: 10
                        spacing: 12

                        Rectangle {
                            visible: model.hasImage && model.thumbnailPath !== ""
                            Layout.preferredWidth: 60
                            Layout.preferredHeight: 60
                            Layout.alignment: Qt.AlignTop
                            radius: 4
                            color: "#2d2d2d"
                            clip: true

                            Image {
                                anchors.fill: parent
                                anchors.margins: 2
                                source: model.thumbnailPath !== ""
                                    ? "file://" + model.thumbnailPath
                                    : ""
                                fillMode: Image.PreserveAspectCrop
                                asynchronous: true
                                cache: true
                            }
                        }

                        ColumnLayout {
                            id: contentColumn
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            spacing: 4

                            Text {
                                text: model.title || "Untitled"
                                color: "#ffffff"
                                font.bold: true
                                elide: Text.ElideRight
                                Layout.fillWidth: true
                            }

                            Text {
                                text: model.body || ""
                                color: "#9ca3af"
                                font.pixelSize: 12
                                maximumLineCount: 2
                                elide: Text.ElideRight
                                wrapMode: Text.WordWrap
                                Layout.fillWidth: true
                            }
                        }

                        // Fixed-width right column for star button - always visible
                        Rectangle {
                            Layout.preferredWidth: 40
                            Layout.fillHeight: true
                            Layout.leftMargin: 8
                            color: "transparent"

                            Text {
                                text: model.starred ? "★" : "☆"
                                color: "#fbbf24"
                                font.pixelSize: 18
                                anchors.centerIn: parent

                                MouseArea {
                                    anchors.fill: parent
                                    onClicked: {
                                        ipcClient.star(clipId, !starred)
                                        starred = !starred
                                    }
                                }
                            }
                        }
                    }
                }
            }


            GridView {
                id: contentGrid
                focus: false
                clip: true
                cellWidth: gridSettings.grid ? (contentGrid.width / gridSettings.grid.columns) : 180
                cellHeight: gridSettings.grid ? gridSettings.grid.thumb_size : 180
                model: ListModel { id: galleryModel }
                topMargin: 6

                delegate: Rectangle {
                    
                    width: contentGrid.cellWidth - 6
                    height: contentGrid.cellHeight - 6
                    radius: 6
                    color: "#2d2d2d"
                    border.color: isSelected(model.clipId) ? "#974ade" : (contentGrid.currentIndex === index ? "#974ade" : "transparent")
                    border.width: 3
                    clip: true

                    MouseArea {
                        anchors.fill: parent
                        onClicked: {
                            contentGrid.currentIndex = index
                            if (!(mouse.modifiers & Qt.ShiftModifier)) {
                                clearSelection()
                                selectionAnchor = -1
                            }
                            ipcClient.copy(clipId)
                        }
                    }

                    Image {
                        anchors.fill: parent
                        anchors.margins: 4
                        source: model.thumbnailPath !== ""
                            ? "file://" + model.thumbnailPath
                            : ""
                        fillMode: Image.PreserveAspectFit
                        asynchronous: true
                        cache: true
                    }

                    Text {
                        text: starred ? "★" : ""
                        color: "#fbbf24"
                        anchors.top: parent.top
                        anchors.right: parent.right
                        anchors.margins: 6
                        font.pixelSize: 18
                    }
                }
            }
        }


        Rectangle {
            Layout.fillWidth: true
            Layout.preferredHeight: 28
            color: "#2d2d2d"

            Text {
                id: statusBar
                anchors.centerIn: parent
                text: "Ready"
                color: "#9ca3af"
                font.pixelSize: 11
            }
        }
    }
}
