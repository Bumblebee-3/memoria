# Memoria

## The (objectively) better clipboard manager.
## Demo


https://github.com/user-attachments/assets/71ac87fb-e52a-41bc-911d-84bb94758245



### Features:
- Clipboard cleanup
- Starring/favourites
- Gallery view
- image previews
- searching
- keyboard navigation
    arrow keys to navigate, / to search, enter to select and esc to close, del to delete and s to star.




Project Structure:

This repo contains two pieces:

- `memoria-daemon`: a background service (Rust) that stores/serves memos.
- `memoria-ui`: a Qt/QML desktop UI that talks to the daemon (IPC).


```
Memoria/
├── .gitignore
├── README.md
├── config.example.toml
├── PKGBUILD
├── memoria-daemon/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── memoria-daemon.service
│   └── src/
│       ├── main.rs
│       ├── clipboard.rs
│       ├── config.rs
│       ├── db.rs
│       ├── ipc.rs
│       └── retention.rs
└── memoria-ui/
    ├── CMakeLists.txt
    ├── qml/
    │   ├── main.qml
    │   └── qml.qrc
    └── src/
        ├── main.cpp
        ├── ipcclient.h
        └── ipcclient.cpp
```

# Installation:

1. Download
```
git clone https://github.com/Bumblebee-3/memoria.git
cd ./memoria
```
2. Install
```
makepkg -si
```
and then
```
systemctl --user daemon-reload
systemctl --user enable --now memoria-daemon
```
3. Config
copy `config.example.toml` to `.config/memoria/config.toml`.

3. Test
```
memoria-ui
```
