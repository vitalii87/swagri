package com.swagri.android

object NativeBridge {
    init {
        System.loadLibrary("swagri_agent")
    }

    external fun nativeStart(
        dataDir: String,
        name: String,
        maxCpuPercent: Float,
        maxMemoryPercent: Float,
        artifactQuotaBytes: Long,
    ): Boolean

    external fun nativeSend(command: String): Boolean
    external fun nativeStop(): Boolean
    external fun nativeIsRunning(): Boolean
    external fun nativePoll(): String

    external fun nativeUpdateEnvironment(
        batteryPercent: Int,
        charging: Boolean,
        thermalStatus: Int,
        unmeteredNetwork: Boolean,
    )
}
