# Qt Test suites. They run the real engine executable, so they need PDFium (`cargo xtask pdfium
# fetch`) and fail, never skip, without it.
# Every Qt Test executable writes its report to a file in `test-logs/` as well. On Windows its
# console output does not reach CTest, so without the file a failure would say only "Failed"; CI
# prints these files when a step fails.
set(VELLORA_TEST_LOG_DIR "${CMAKE_BINARY_DIR}/test-logs")
file(MAKE_DIRECTORY "${VELLORA_TEST_LOG_DIR}")

function(vellora_add_test name source)
  qt_add_executable(${name} ${source})
  target_link_libraries(${name} PRIVATE vellora_shell Qt6::Test)
  target_compile_definitions(${name} PRIVATE
    VELLORA_GOLDEN_PDF="${VELLORA_ROOT}/crates/engine/tests/fixtures/golden.pdf"
    # One page, user password `user-pw` (made by crates/cos/tests/fixtures/encryption/generate.py).
    VELLORA_PROTECTED_PDF="${VELLORA_ROOT}/crates/cos/tests/fixtures/encryption/r6-aes-256-user-password.pdf")
  vellora_stage_runtime(${name})
  add_test(NAME ${name} COMMAND ${name} -o "${VELLORA_TEST_LOG_DIR}/${name}.log,txt")
  # QTEST_FUNCTION_TIMEOUT makes Qt name the test function that hangs, before CTest's timeout kills
  # the whole executable without saying which one it was.
  set_tests_properties(${name} PROPERTIES
    ENVIRONMENT "QT_QPA_PLATFORM=offscreen;QTEST_FUNCTION_TIMEOUT=90000" TIMEOUT 150)
endfunction()

vellora_add_test(tst_engine_session tst_engine_session.cpp)
vellora_add_test(tst_main_window tst_main_window.cpp)
vellora_add_test(tst_view_modes tst_view_modes.cpp)
vellora_add_test(tst_thumbnails tst_thumbnails.cpp)
# The frame-time check of the sidebar: a timing test, so it is a file of its own. It is disabled on
# the macOS CI runner, whose handlers and repaints go over 8 ms by up to 20 ms for code that takes
# 0.5 ms elsewhere (app/README.md, "macOS and the canvas test"); set VELLORA_TEST_FRAME_TIME=1 to run
# it, on real hardware.
vellora_add_test(tst_thumbnails_frame_time tst_thumbnails_frame_time.cpp)
if(APPLE AND NOT "$ENV{VELLORA_TEST_FRAME_TIME}" STREQUAL "1")
  set_tests_properties(tst_thumbnails_frame_time PROPERTIES DISABLED TRUE)
endif()
vellora_add_test(tst_navigation tst_navigation.cpp)
vellora_add_test(tst_tabs tst_tabs.cpp)
vellora_add_test(tst_about tst_about.cpp)
vellora_add_test(tst_logging tst_logging.cpp)
vellora_add_test(tst_crash tst_crash.cpp)
# The crash tests start the real application (`vellora --crash-test`) and use it as the monitor.
add_dependencies(tst_crash vellora)
target_compile_definitions(tst_crash PRIVATE VELLORA_APP_PATH="$<TARGET_FILE:vellora>")
vellora_add_test(tst_settings tst_settings.cpp)
vellora_add_test(tst_single_instance tst_single_instance.cpp)
vellora_add_test(tst_page_layout tst_page_layout.cpp)
vellora_add_test(tst_canvas_controller tst_canvas_controller.cpp)
vellora_add_test(tst_canvas_view tst_canvas_view.cpp)
vellora_add_test(tst_commands tst_commands.cpp)
vellora_add_test(tst_ui_watchdog tst_ui_watchdog.cpp)
vellora_add_test(tst_engine_recovery tst_engine_recovery.cpp)
vellora_add_test(tst_diagnostics_script tst_diagnostics_script.cpp)

# The hidden `--diagnostics-script` flag of the app itself: runs the scripted session on a small
# document (no GPU needed; the offscreen platform draws nothing) and exits 0 with its report.
add_test(NAME vellora_diagnostics_script
         COMMAND vellora --diagnostics-script scroll-zoom
                 "${VELLORA_ROOT}/crates/engine/tests/fixtures/golden.pdf")
set_tests_properties(vellora_diagnostics_script PROPERTIES
  ENVIRONMENT "QT_QPA_PLATFORM=offscreen" TIMEOUT 150
  PASS_REGULAR_EXPRESSION "diagnostics-script scroll-zoom: 2 pages, 20 zooms;")
add_test(NAME vellora_diagnostics_script_unknown
         COMMAND vellora --diagnostics-script no-such-scenario)
set_tests_properties(vellora_diagnostics_script_unknown PROPERTIES
  ENVIRONMENT "QT_QPA_PLATFORM=offscreen" TIMEOUT 60
  PASS_REGULAR_EXPRESSION "unknown --diagnostics-script 'no-such-scenario'")
# Drawing needs a platform that can run QRhi: the real one on Windows and macOS (their runners
# have a software or virtual GPU), and an X server with Mesa's software OpenGL on Linux.
qt_add_executable(tst_canvas_render tst_canvas_render.cpp)
target_link_libraries(tst_canvas_render PRIVATE vellora_shell Qt6::Test)
target_compile_definitions(tst_canvas_render PRIVATE
  VELLORA_GOLDEN_PDF="${VELLORA_ROOT}/crates/engine/tests/fixtures/golden.pdf")
vellora_stage_runtime(tst_canvas_render)
if(UNIX AND NOT APPLE)
  find_program(VELLORA_XVFB_RUN xvfb-run)
  if(VELLORA_XVFB_RUN)
    add_test(NAME tst_canvas_render
             COMMAND ${VELLORA_XVFB_RUN} -a -s "-screen 0 1280x1024x24" $<TARGET_FILE:tst_canvas_render>
                     -o "${VELLORA_TEST_LOG_DIR}/tst_canvas_render.log,txt")
    set_tests_properties(tst_canvas_render PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=xcb;QTEST_FUNCTION_TIMEOUT=360000")
  else()
    message(WARNING "xvfb-run not found: tst_canvas_render (GPU drawing) is not registered")
  endif()
else()
  add_test(NAME tst_canvas_render
           COMMAND tst_canvas_render -o "${VELLORA_TEST_LOG_DIR}/tst_canvas_render.log,txt")
  if(APPLE AND NOT "$ENV{VELLORA_TEST_GPU}" STREQUAL "1")
    # On the macOS CI runners (a paravirtualised Metal GPU) this test is not stuck but extremely
    # slow, so it runs into its timeouts, and its frame-time check could not pass. Evidence in
    # app/README.md ("macOS and the canvas test"). Run it on real hardware or set VELLORA_TEST_GPU=1.
    set_tests_properties(tst_canvas_render PROPERTIES DISABLED TRUE)
  endif()
  set_tests_properties(tst_canvas_render PROPERTIES ENVIRONMENT "QTEST_FUNCTION_TIMEOUT=360000")
endif()
if(TEST tst_canvas_render)
  set_tests_properties(tst_canvas_render PROPERTIES TIMEOUT 600)
endif()
