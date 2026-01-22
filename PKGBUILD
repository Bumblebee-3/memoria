# Maintainer: Bumblebee-3

pkgbase=memoria
pkgname=(memoria-daemon memoria-ui)
pkgver=1.0.0
pkgrel=1
arch=(x86_64)
url="https://github.com/Bumblebee-3/memoria"
license=(MIT)

makedepends=(
  cargo
  cmake
  ninja
  qt6-base
  qt6-declarative
  sqlite
)

source=("memoria::git+file://${PWD}")
sha256sums=('SKIP')

build() {
  cd "$srcdir/memoria"

  cd memoria-daemon
  cargo build --release --locked
  cd ..

  cd memoria-ui
  cmake -S . -B build \
    -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/usr
  cmake --build build
}

package_memoria-daemon() {
  pkgdesc="memoria daemon (systemd user service)"
  depends=(
    gcc-libs
    sqlite
  )


  cd "$srcdir/memoria"

  install -Dm755 memoria-daemon/target/release/memoria-daemon \
    "$pkgdir/usr/bin/memoria-daemon"

  install -Dm644 memoria-daemon/memoria-daemon.service \
    "$pkgdir/usr/lib/systemd/user/memoria-daemon.service"


}

package_memoria-ui() {
  pkgdesc="memoria Qt6 UI"
  depends=(
    gcc-libs
    qt6-base
    qt6-declarative
    memoria-daemon
  )

  cd "$srcdir/memoria/memoria-ui"
  DESTDIR="$pkgdir" cmake --install build
}



