package com.swagri.android

import android.Manifest
import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.database.Cursor
import android.graphics.Color
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import android.text.Editable
import android.text.TextWatcher
import android.view.View
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import java.io.File
import java.text.SimpleDateFormat
import java.util.ArrayDeque
import java.util.Date
import java.util.Locale

class MainActivity : Activity() {
    companion object {
        private const val PICK_ARTIFACT = 1401
        private const val MAX_LOG_LINES = 1_000
    }

    private val handler = Handler(Looper.getMainLooper())
    private val logs = ArrayDeque<String>()
    private val peers = linkedSetOf<String>()
    private lateinit var stateView: TextView
    private lateinit var identityView: TextView
    private lateinit var resourcesView: TextView
    private lateinit var peersView: TextView
    private lateinit var logView: TextView
    private lateinit var logScroll: ScrollView
    private lateinit var searchInput: EditText
    private lateinit var peerInput: EditText
    private lateinit var nodeNameInput: EditText
    private lateinit var cpuInput: EditText
    private lateinit var memoryInput: EditText
    private lateinit var storageInput: EditText

    private val pollEvents = object : Runnable {
        override fun run() {
            runCatching { NativeBridge.nativePoll() }
                .getOrDefault("")
                .lineSequence()
                .filter(String::isNotBlank)
                .forEach(::acceptEvent)
            updateRunningState()
            handler.postDelayed(this, 500L)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        title = "Swagri Android Agent"
        setContentView(buildUi())
        loadSettings()
        requestNotificationPermission()
        addLocalLog("INFO", "GUI", "Android debugger ready")
    }

    override fun onResume() {
        super.onResume()
        handler.post(pollEvents)
    }

    override fun onPause() {
        handler.removeCallbacks(pollEvents)
        super.onPause()
    }

    private fun buildUi(): View {
        val page = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(14), dp(12), dp(14), dp(24))
        }
        page.addView(TextView(this).apply {
            text = "Swagri Android Agent · 0.14.1-alpha"
            textSize = 22f
            setTextColor(Color.rgb(16, 90, 68))
        })
        stateView = label("STOPPED · contribution session is not active")
        identityView = label("Peer ID: —")
        resourcesView = label("Mobile resources: waiting for the first sample")
        page.addView(stateView)
        page.addView(identityView)
        page.addView(resourcesView)

        page.addView(section("Safe contribution settings"))
        nodeNameInput = input("Node name")
        cpuInput = input("Maximum CPU %, default 25")
        memoryInput = input("Maximum RAM %, default 25")
        storageInput = input("Artifact cache MiB, default 256")
        page.addView(nodeNameInput)
        page.addView(horizontal(cpuInput, memoryInput))
        page.addView(storageInput)
        page.addView(horizontal(
            actionButton("Start agent") { startAgent() },
            actionButton("Stop agent") { stopAgent() },
        ))
        page.addView(label("Contribution works at 50% battery or above without a charger. Below 50%, connect power. Unmetered Wi-Fi and safe temperature are always required."))

        page.addView(section("Peers and trust"))
        peersView = label("Found agents: 0")
        peerInput = input("Peer ID or /ip4/.../udp/.../quic-v1 address")
        page.addView(peersView)
        page.addView(peerInput)
        page.addView(horizontal(
            actionButton("Find / refresh") { send("peers") },
            actionButton("Connect") { peerCommand("connect", allowAddress = true) },
            actionButton("Trust") { peerCommand("trust") },
        ))
        page.addView(horizontal(
            actionButton("Echo test") { peerCommand("echo", suffix = " hello-from-android") },
            actionButton("Resources") { peerCommand("resources") },
            actionButton("Matrix 192") { peerCommand("matrix", suffix = " 192") },
        ))

        page.addView(section("Artifacts"))
        page.addView(horizontal(
            actionButton("Import file") { chooseArtifact() },
            actionButton("Local files") { send("artifact-list") },
            actionButton("Storage status") { send("artifact-status") },
        ))

        page.addView(section("Diagnostic log"))
        searchInput = input("Search log")
        searchInput.addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence?, start: Int, count: Int, after: Int) = Unit
            override fun onTextChanged(s: CharSequence?, start: Int, before: Int, count: Int) = renderLog()
            override fun afterTextChanged(s: Editable?) = Unit
        })
        page.addView(searchInput)
        page.addView(horizontal(
            actionButton("Copy visible") { copyVisibleLog() },
            actionButton("Clear") {
                logs.clear()
                renderLog()
            },
        ))
        logView = TextView(this).apply {
            typeface = android.graphics.Typeface.MONOSPACE
            textSize = 11f
            setTextIsSelectable(true)
            setPadding(dp(8), dp(8), dp(8), dp(8))
            setBackgroundColor(Color.rgb(245, 248, 247))
        }
        logScroll = ScrollView(this).apply {
            addView(logView)
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                dp(320),
            )
        }
        page.addView(logScroll)

        page.addView(section("Technical command"))
        val commandInput = input("help, peers, auto-matrix 320, artifact-list…")
        page.addView(horizontal(
            commandInput,
            actionButton("Send") {
                send(commandInput.text.toString())
                commandInput.text.clear()
            },
        ))

        return ScrollView(this).apply { addView(page) }
    }

    private fun startAgent() {
        saveSettings()
        val intent = Intent(this, AgentService::class.java).setAction(AgentService.ACTION_START)
        startForegroundService(intent)
        addLocalLog("INFO", "GUI", "Contribution session requested")
    }

    private fun stopAgent() {
        startService(Intent(this, AgentService::class.java).setAction(AgentService.ACTION_STOP))
        addLocalLog("INFO", "GUI", "Stop requested")
    }

    private fun send(command: String) {
        val value = command.trim()
        if (value.isEmpty()) return
        val accepted = runCatching { NativeBridge.nativeSend(value) }.getOrDefault(false)
        addLocalLog(if (accepted) "INFO" else "ERROR", "CMD", value)
        if (!accepted) toast("Start the agent first")
    }

    private fun peerCommand(command: String, suffix: String = "", allowAddress: Boolean = false) {
        val peer = peerInput.text.toString().trim()
        if (peer.isEmpty()) {
            toast("Enter or tap a Peer ID")
            return
        }
        val actual = if (allowAddress && peer.startsWith("/")) "dial" else command
        send("$actual $peer$suffix")
    }

    private fun acceptEvent(raw: String) {
        val fields = raw.split('\t')
        val kind = fields.getOrNull(1) ?: "EVENT"
        val level = when {
            kind.endsWith("FAILED") || kind.contains("ERROR") || kind.contains("REJECTED") -> "ERROR"
            kind.contains("DISCONNECTED") || kind.contains("PAUSED") || kind.contains("PROVIDER_FAILED") -> "WARN"
            else -> "EVENT"
        }
        addLocalLog(level, "AGENT", raw)
        when (kind) {
            "STARTED" -> identityView.text = "Peer ID: ${fields.getOrNull(2) ?: "—"}"
            "PEER_DISCOVERED", "PEER_CONNECTED" -> fields.getOrNull(2)?.let {
                peers.add(it)
                peersView.text = "Found agents: ${peers.size}\n" + peers.joinToString("\n")
                if (peerInput.text.isBlank()) peerInput.setText(it)
            }
            "PEER_DISCONNECTED" -> fields.getOrNull(2)?.let {
                peersView.text = "Found agents: ${peers.size} · disconnected ${shortPeer(it)}"
            }
            "LOCAL_RESOURCES" -> {
                val cpu = fields.getOrNull(11) ?: "?"
                val ram = fields.getOrNull(10)?.toLongOrNull()?.let(::formatBytes) ?: "?"
                val score = fields.getOrNull(19) ?: "?"
                val paused = fields.getOrNull(20) == "true"
                val battery = fields.getOrNull(21) ?: "?"
                val charging = fields.getOrNull(22) ?: "?"
                val thermal = fields.getOrNull(23) ?: "?"
                resourcesView.text = "CPU $cpu% · free RAM $ram · score $score · " +
                    "battery $battery% · charging $charging · thermal $thermal · " +
                    if (paused) "PAUSED" else "CONTRIBUTING"
            }
        }
    }

    private fun addLocalLog(level: String, source: String, message: String) {
        val timestamp = SimpleDateFormat("yyyy-MM-dd HH:mm:ss.SSS", Locale.US).format(Date())
        logs.addLast("$timestamp  $level  $source  $message")
        while (logs.size > MAX_LOG_LINES) logs.removeFirst()
        renderLog()
    }

    private fun renderLog() {
        if (!::logView.isInitialized) return
        val query = if (::searchInput.isInitialized) searchInput.text.toString().trim() else ""
        val visible = logs.filter { query.isEmpty() || it.contains(query, ignoreCase = true) }
        logView.text = visible.joinToString("\n")
        logScroll.post { logScroll.fullScroll(View.FOCUS_DOWN) }
    }

    private fun copyVisibleLog() {
        getSystemService(ClipboardManager::class.java).setPrimaryClip(
            ClipData.newPlainText("Swagri Android log", logView.text),
        )
        toast("Visible log copied")
    }

    private fun updateRunningState() {
        val running = runCatching { NativeBridge.nativeIsRunning() }.getOrDefault(false)
        stateView.text = if (running) "RUNNING · foreground contribution session" else "STOPPED"
        stateView.setTextColor(if (running) Color.rgb(0, 150, 80) else Color.rgb(190, 45, 45))
    }

    private fun chooseArtifact() {
        startActivityForResult(
            Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                addCategory(Intent.CATEGORY_OPENABLE)
                type = "*/*"
            },
            PICK_ARTIFACT,
        )
    }

    @Deprecated("Deprecated by Android but retained for API 29 compatibility")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode != PICK_ARTIFACT || resultCode != RESULT_OK) return
        val uri = data?.data ?: return
        Thread {
            runCatching { copyArtifact(uri) }
                .onSuccess { path -> runOnUiThread { send("artifact-import $path") } }
                .onFailure { error -> runOnUiThread { toast("Import failed: ${error.message}") } }
        }.start()
    }

    private fun copyArtifact(uri: Uri): String {
        val directory = filesDir.resolve("imports").apply(File::mkdirs)
        val displayName = queryDisplayName(uri)
            .replace(Regex("[^A-Za-z0-9._-]"), "_")
            .take(96)
            .ifBlank { "artifact.bin" }
        val target = directory.resolve("${System.currentTimeMillis()}-$displayName")
        contentResolver.openInputStream(uri).use { input ->
            requireNotNull(input) { "Cannot open selected file" }
            target.outputStream().use(input::copyTo)
        }
        return target.absolutePath
    }

    private fun queryDisplayName(uri: Uri): String {
        var cursor: Cursor? = null
        return try {
            cursor = contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
            if (cursor != null && cursor.moveToFirst()) cursor.getString(0) else "artifact.bin"
        } finally {
            cursor?.close()
        }
    }

    private fun loadSettings() {
        val preferences = getSharedPreferences("swagri", MODE_PRIVATE)
        nodeNameInput.setText(preferences.getString("node_name", "android-${Build.MODEL}"))
        cpuInput.setText(preferences.getFloat("max_cpu", 25f).toInt().toString())
        memoryInput.setText(preferences.getFloat("max_memory", 25f).toInt().toString())
        storageInput.setText(preferences.getInt("storage_mib", 256).toString())
    }

    private fun saveSettings() {
        getSharedPreferences("swagri", MODE_PRIVATE).edit()
            .putString("node_name", nodeNameInput.text.toString().trim())
            .putFloat("max_cpu", cpuInput.text.toString().toFloatOrNull()?.coerceIn(1f, 100f) ?: 25f)
            .putFloat("max_memory", memoryInput.text.toString().toFloatOrNull()?.coerceIn(1f, 100f) ?: 25f)
            .putInt("storage_mib", storageInput.text.toString().toIntOrNull()?.coerceIn(32, 4096) ?: 256)
            .apply()
    }

    private fun requestNotificationPermission() {
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 140)
        }
    }

    private fun section(text: String) = TextView(this).apply {
        this.text = text
        textSize = 18f
        setPadding(0, dp(18), 0, dp(6))
        setTextColor(Color.rgb(16, 90, 68))
    }

    private fun label(text: String) = TextView(this).apply {
        this.text = text
        textSize = 14f
        setPadding(0, dp(4), 0, dp(4))
    }

    private fun input(hint: String) = EditText(this).apply {
        this.hint = hint
        isSingleLine = true
        layoutParams = LinearLayout.LayoutParams(
            LinearLayout.LayoutParams.MATCH_PARENT,
            LinearLayout.LayoutParams.WRAP_CONTENT,
        )
    }

    private fun actionButton(text: String, action: () -> Unit) = Button(this).apply {
        this.text = text
        setOnClickListener { action() }
        layoutParams = LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
    }

    private fun horizontal(vararg views: View) = LinearLayout(this).apply {
        orientation = LinearLayout.HORIZONTAL
        views.forEach { view ->
            val parameters = view.layoutParams as? LinearLayout.LayoutParams
                ?: LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f)
            parameters.width = 0
            parameters.weight = 1f
            parameters.setMargins(dp(2), dp(2), dp(2), dp(2))
            addView(view, parameters)
        }
    }

    private fun dp(value: Int) = (value * resources.displayMetrics.density).toInt()
    private fun toast(message: String) = Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
    private fun shortPeer(peer: String) = if (peer.length <= 18) peer else "${peer.take(10)}…${peer.takeLast(6)}"
    private fun formatBytes(bytes: Long): String = "%.2f GiB".format(Locale.US, bytes / 1024.0 / 1024.0 / 1024.0)
}
