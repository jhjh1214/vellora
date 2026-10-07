# Qt Test suites. They run the real engine executable, so they need PDFium (`cargo xtask pdfium
# fetch`) and fail, never skip, without it.
function(vellora_add_test name source)
  qt_add_executable(${name} ${source})
  target_link_libraries(${name} PRIVATE vellora_shell Qt6::Test)
  target_compile_definitions(${name} PRIVATE
    VELLORA_GOLDEN_PDF="${VELLORA_ROOT}/crates/engine/tests/fixtures/golden.pdf")
  vellora_stage_runtime(${name})
  add_test(NAME ${name} COMMAND ${name})
  set_tests_properties(${name} PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen" TIMEOUT 120)
endfunction()

vellora_add_test(tst_engine_session tst_engine_session.cpp)
vellora_add_test(tst_main_window tst_main_window.cpp)
vellora_add_test(tst_page_layout tst_page_layout.cpp)
vellora_add_test(tst_canvas_controller tst_canvas_controller.cpp)
vellora_add_test(tst_canvas_view tst_canvas_view.cpp)

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
             COMMAND ${VELLORA_XVFB_RUN} -a -s "-screen 0 1280x1024x24" $<TARGET_FILE:tst_canvas_render>)
    set_tests_properties(tst_canvas_render PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=xcb")
  else()
    message(WARNING "xvfb-run not found: tst_canvas_render (GPU drawing) is not registered")
  endif()
else()
  add_test(NAME tst_canvas_render COMMAND tst_canvas_render)
endif()
if(TEST tst_canvas_render)
  set_tests_properties(tst_canvas_render PROPERTIES TIMEOUT 120)
endif()
