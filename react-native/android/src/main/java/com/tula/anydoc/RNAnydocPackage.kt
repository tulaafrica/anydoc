package com.tula.anydoc

import com.facebook.react.ReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.uimanager.ViewManager
import com.margelo.nitro.tula.anydoc.RNAnydocOnLoad

/**
 * Autolinking entry point. Nitro HybridObjects register through the C++
 * HybridObjectRegistry (see nitrogen's RNAnydocOnLoad.cpp), so this package
 * contributes no React modules — its whole job is existing (so React Native
 * autolinking discovers the library) and loading the native library.
 */
class RNAnydocPackage : ReactPackage {
  init {
    RNAnydocOnLoad.initializeNative()
  }

  override fun createNativeModules(reactContext: ReactApplicationContext): List<NativeModule> =
    emptyList()

  override fun createViewManagers(reactContext: ReactApplicationContext): List<ViewManager<*, *>> =
    emptyList()
}
