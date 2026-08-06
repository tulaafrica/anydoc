// JNI entry point. System.loadLibrary("RNAnydoc") lands here, and this is
// what registers the DocumentConverter constructor with Nitro's
// HybridObjectRegistry (see nitrogen's RNAnydocOnLoad.cpp).
#include <jni.h>
#include "RNAnydocOnLoad.hpp"

extern "C" JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM* vm, void*) {
  return margelo::nitro::tula::anydoc::initialize(vm);
}
