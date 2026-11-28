cmake_minimum_required(VERSION 3.16)
project(cmake-hello VERSION 1.4.0 LANGUAGES C)
include(GNUInstallDirs)
find_package(ZLIB REQUIRED)

# A library of its own, a data file found where it's installed, and zlib.
add_library(greet SHARED greet.c)
add_executable(cmake-hello main.c)
target_link_libraries(cmake-hello greet ZLIB::ZLIB)
target_compile_definitions(cmake-hello PRIVATE
  DATADIR="${CMAKE_INSTALL_FULL_DATADIR}/cmake-hello"
  VERSION="${PROJECT_VERSION}")
install(TARGETS cmake-hello greet)
install(FILES greeting.txt DESTINATION ${CMAKE_INSTALL_DATADIR}/cmake-hello)
