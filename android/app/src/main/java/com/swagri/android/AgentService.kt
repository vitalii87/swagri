package com.swagri.android

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager

class AgentService : Service() {
    companion object {
        const val ACTION_START = "com.swagri.android.START"
        const val ACTION_STOP = "com.swagri.android.STOP"
        private const val CHANNEL_ID = "swagri-agent"
        private const val NOTIFICATION_ID = 140
        private const val MAX_SESSION_MS = 5L * 60L * 60L * 1000L + 45L * 60L * 1000L
    }

    private val handler = Handler(Looper.getMainLooper())
    private var multicastLock: WifiManager.MulticastLock? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var startedAt = 0L

    private val environmentTick = object : Runnable {
        override fun run() {
            updateEnvironment()
            if (startedAt > 0L && System.currentTimeMillis() - startedAt >= MAX_SESSION_MS) {
                NativeBridge.nativeStop()
                stopSelf()
                return
            }
            handler.postDelayed(this, 10_000L)
        }
    }

    override fun onCreate() {
        super.onCreate()
        createNotificationChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            runCatching { NativeBridge.nativeStop() }
            stopSelf()
            return START_NOT_STICKY
        }

        startForeground(NOTIFICATION_ID, notification("Starting the local P2P node…"))
        if (!NativeBridge.nativeIsRunning()) {
            val preferences = getSharedPreferences("swagri", MODE_PRIVATE)
            val name = preferences.getString("node_name", null)
                ?.trim()
                ?.takeIf(String::isNotEmpty)
                ?: "android-${android.os.Build.MODEL}"
            val cpu = preferences.getFloat("max_cpu", 25f).coerceIn(1f, 100f)
            val memory = preferences.getFloat("max_memory", 25f).coerceIn(1f, 100f)
            val storageMiB = preferences.getInt("storage_mib", 256).coerceIn(32, 4096)
            val started = runCatching {
                NativeBridge.nativeStart(
                    filesDir.resolve("agent").absolutePath,
                    name,
                    cpu,
                    memory,
                    storageMiB.toLong() * 1024L * 1024L,
                    applicationInfo.sourceDir,
                )
            }.getOrDefault(false)
            if (!started) {
                stopSelf()
                return START_NOT_STICKY
            }
            startedAt = System.currentTimeMillis()
        }

        acquireMulticast()
        handler.removeCallbacks(environmentTick)
        handler.post(environmentTick)
        return START_NOT_STICKY
    }

    private fun updateEnvironment() {
        val batteryIntent = registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val level = batteryIntent?.getIntExtra(BatteryManager.EXTRA_LEVEL, -1) ?: -1
        val scale = batteryIntent?.getIntExtra(BatteryManager.EXTRA_SCALE, 100) ?: 100
        val batteryPercent = if (level >= 0 && scale > 0) level * 100 / scale else -1
        val status = batteryIntent?.getIntExtra(BatteryManager.EXTRA_STATUS, -1) ?: -1
        val charging = status == BatteryManager.BATTERY_STATUS_CHARGING ||
            status == BatteryManager.BATTERY_STATUS_FULL
        val powerManager = getSystemService(PowerManager::class.java)
        val thermal = powerManager.currentThermalStatus
        val connectivity = getSystemService(ConnectivityManager::class.java)
        val capabilities = connectivity.getNetworkCapabilities(connectivity.activeNetwork)
        val unmetered = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) == true &&
            capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)

        NativeBridge.nativeUpdateEnvironment(batteryPercent, charging, thermal, unmetered)
        val contributionAllowed = batteryPercent >= 0 &&
            (batteryPercent >= 50 || charging) &&
            unmetered &&
            thermal < PowerManager.THERMAL_STATUS_SEVERE
        updateWakeLock(contributionAllowed)
        val state = "Battery ${if (batteryPercent >= 0) "$batteryPercent%" else "?"} · " +
            "thermal $thermal · ${if (unmetered) "Wi-Fi" else "paused network"}"
        getSystemService(NotificationManager::class.java)
            .notify(NOTIFICATION_ID, notification(state))
    }

    private fun acquireMulticast() {
        if (multicastLock?.isHeld == true) return
        multicastLock = getSystemService(WifiManager::class.java)
            .createMulticastLock("swagri-mdns")
            .apply {
                setReferenceCounted(false)
                acquire()
            }
    }

    private fun updateWakeLock(needed: Boolean) {
        if (needed && wakeLock?.isHeld != true) {
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Swagri::AgentSession")
                .apply {
                    setReferenceCounted(false)
                    acquire(MAX_SESSION_MS)
                }
        } else if (!needed && wakeLock?.isHeld == true) {
            wakeLock?.release()
        }
    }

    private fun createNotificationChannel() {
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(
                CHANNEL_ID,
                "Swagri agent",
                NotificationManager.IMPORTANCE_LOW,
            ),
        )
    }

    private fun notification(text: String): Notification {
        val openIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val stopIntent = PendingIntent.getService(
            this,
            1,
            Intent(this, AgentService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.stat_notify_sync)
            .setContentTitle("Swagri Android Agent")
            .setContentText(text)
            .setContentIntent(openIntent)
            .setOngoing(true)
            .addAction(
                Notification.Action.Builder(
                    android.R.drawable.ic_media_pause,
                    "Stop",
                    stopIntent,
                ).build(),
            )
            .build()
    }

    override fun onDestroy() {
        handler.removeCallbacks(environmentTick)
        runCatching { NativeBridge.nativeStop() }
        if (multicastLock?.isHeld == true) multicastLock?.release()
        if (wakeLock?.isHeld == true) wakeLock?.release()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
