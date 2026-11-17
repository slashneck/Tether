package io.github.slashneck.tether

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.PowerManager
import org.json.JSONArray
import org.json.JSONObject
import java.io.IOException
import java.net.InetAddress
import java.net.ServerSocket

/**
 * Runs while Tether is on. Listens for the PC, streams the camera to it, survives the screen turning off.
 *
 * Only listens on the phone's own loopback address: over USB the PC reaches it through `adb forward`, and nothing on
 * the Wi‑Fi network can connect. (Wi‑Fi mode with pairing comes in milestone 4.)
 */
class StreamService : Service() {
    private val main = Handler(Looper.getMainLooper())
    private lateinit var streamer: CameraStreamer
    private var server: ServerSocket? = null
    private var acceptThread: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null

    @Volatile private var link: Link? = null
    @Volatile private var settings = StreamSettings()
    /** The last SPS/PPS, re-sent to a PC that connects while the encoder is already running. */
    @Volatile private var codecConfig: ByteArray? = null

    /** After the PC goes away, keep the camera open briefly so a quick replug resumes instantly. */
    private val idleStop = Runnable { streamer.stop() }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        streamer = CameraStreamer(this, ::onEncoded) { running, actual, error ->
            if (actual != null) settings = actual
            Bus.update { it.copy(streaming = running, settings = actual ?: it.settings, error = error ?: "") }
            sendStatus()
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }
        goForeground()
        if (server == null) startServer()
        if (wakeLock == null) {
            wakeLock = getSystemService(PowerManager::class.java)
                .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "Tether:stream")
                .apply { acquire() }
        }
        Bus.update { it.copy(serviceOn = true, error = "") }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        instance = null
        main.removeCallbacks(idleStop)
        runCatching { server?.close() }
        server = null
        link?.close()
        streamer.release()
        statusSender.shutdown()
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        Bus.update { TetherState(closedByPcAt = it.closedByPcAt) }
        super.onDestroy()
    }

    private fun goForeground() {
        val nm = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, "Tether", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Shows while Tether is ready to stream"
                    setShowBadge(false)
                },
            )
        }
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this, 1, Intent(this, StreamService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE,
        )
        val n = Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("Tether is on")
            .setContentText("Your camera streams to your PC when it connects")
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "Turn off", stop).build())
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA)
        } else {
            startForeground(NOTIFICATION_ID, n)
        }
    }

    private fun startServer() {
        val s = try {
            ServerSocket(Proto.PORT, 2, InetAddress.getLoopbackAddress()).apply { reuseAddress = true }
        } catch (e: IOException) {
            Bus.update { it.copy(error = "Port ${Proto.PORT} is busy") }
            return
        }
        server = s
        acceptThread = Thread({
            while (!s.isClosed) {
                val socket = try { s.accept() } catch (e: IOException) { break }
                // A new PC connection replaces the old one (e.g. after a replug the old socket may linger).
                link?.close()
                val l = Link(socket)
                link = l
                Thread({ serve(l) }, "tether-link").start()
            }
        }, "tether-accept").apply { start() }
    }

    /** One PC connection, from HELLO until it drops. */
    private fun serve(l: Link) {
        main.removeCallbacks(idleStop)
        l.sendJson(Proto.HELLO, hello())
        var gotSettings = false
        while (true) {
            val (type, payload) = l.read() ?: break
            when (type) {
                Proto.SETTINGS -> {
                    val json = JSONObject(String(payload))
                    val pc = json.optString("pc", "")
                    if (pc.isNotEmpty()) Bus.update { it.copy(pcName = pc) }
                    if (!gotSettings) {
                        gotSettings = true
                        Bus.update { it.copy(connected = true) }
                        // Camera still running from a moment ago: hand the new PC the stream headers right away.
                        codecConfig?.let { cfg -> if (streamer.isRunning) l.sendVideo(Proto.FLAG_CONFIG, 0, cfg, 0, cfg.size) }
                        streamer.requestKeyFrame()
                    }
                    val wanted = settings.merge(json)
                    if (wanted != settings || !streamer.isRunning) {
                        if (!wanted.sameStream(settings)) codecConfig = null
                        settings = wanted
                        streamer.apply(wanted)
                    }
                }
                Proto.KEYFRAME -> streamer.requestKeyFrame()
                Proto.BYE -> {
                    // The PC app quit: stop filming now, close the app, leave nothing running.
                    l.close()
                    Bus.update { it.copy(closedByPcAt = android.os.SystemClock.uptimeMillis()) }
                    main.post { stopSelf() }
                    return
                }
                Proto.PING -> l.send(Proto.PONG, payload)
                else -> Unit // unknown: skip (newer PC app)
            }
        }
        if (link === l) {
            link = null
            Bus.update { it.copy(connected = false) }
            main.postDelayed(idleStop, IDLE_STOP_MS)
        }
    }

    /** A change made on the phone itself (preview, dim, light buttons). The PC hears about it via STATUS. */
    fun changeFromPhone(change: (StreamSettings) -> StreamSettings) {
        val wanted = change(settings)
        settings = wanted
        if (streamer.isRunning) streamer.apply(wanted) else Bus.update { it.copy(settings = wanted) }
        sendStatus()
    }

    fun setPreview(surface: android.view.Surface?) = streamer.setPreview(surface)

    fun previewSize() = streamer.previewSize(settings.cameraId)

    private fun onEncoded(flags: Int, ptsUs: Long, data: ByteArray, length: Int) {
        if (flags and Proto.FLAG_CONFIG != 0) codecConfig = data.copyOf(length)
        link?.sendVideo(flags, ptsUs, data, 0, length)
    }

    private fun hello() = JSONObject().apply {
        put("proto", Proto.VERSION)
        put("app", BuildConfig.VERSION_NAME)
        put("device", "${Build.MANUFACTURER.replaceFirstChar { it.uppercase() }} ${Build.MODEL}")
        put("cameras", JSONArray().apply {
            streamer.cameras().forEach { (id, facing) ->
                put(
                    JSONObject().put("id", id).put("facing", facing).put("torch", streamer.hasTorch(id))
                        .put("maxFps", streamer.maxFps(id)),
                )
            }
        })
    }

    /** Status goes out on its own thread: button taps call this from the main thread, which must never do network. */
    private val statusSender = java.util.concurrent.Executors.newSingleThreadExecutor()

    private fun sendStatus() {
        if (statusSender.isShutdown) return
        statusSender.execute { sendStatusNow() }
    }

    private fun sendStatusNow() {
        val l = link ?: return
        val s = settings
        val battery = getSystemService(BatteryManager::class.java).getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        l.sendJson(Proto.STATUS, JSONObject().apply {
            put("camera", s.cameraId)
            put("width", s.width)
            put("height", s.height)
            put("fps", s.fps)
            put("torch", s.torch)
            put("preview", s.preview)
            put("dim", s.dim)
            put("streaming", streamer.isRunning)
            put("battery", battery)
        })
    }

    companion object {
        private const val CHANNEL = "tether"
        private const val NOTIFICATION_ID = 1
        private const val ACTION_STOP = "io.github.slashneck.tether.STOP"
        private const val IDLE_STOP_MS = 10_000L

        /** The running service, for the activity's preview and buttons (same process). */
        @Volatile
        var instance: StreamService? = null
            private set

        fun start(context: Context) {
            context.startForegroundService(Intent(context, StreamService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, StreamService::class.java))
        }
    }
}
