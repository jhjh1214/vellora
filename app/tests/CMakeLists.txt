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
