cmake_minimum_required(VERSION 3.16)

project(memoria-ui VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

set(CMAKE_AUTOMOC ON)
set(CMAKE_AUTORCC ON)
set(CMAKE_AUTOUIC ON)

find_package(Qt6 REQUIRED COMPONENTS Core Quick)

add_executable(memoria-ui
    src/main.cpp
    src/ipcclient.h
    src/ipcclient.cpp
    qml/qml.qrc
)

target_link_libraries(memoria-ui
    Qt6::Core
    Qt6::Quick
)

install(TARGETS memoria-ui
    RUNTIME DESTINATION bin
)
