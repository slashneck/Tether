package io.github.slashneck.tether

import android.annotation.SuppressLint
import android.content.Context
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.Surface

/**
 * Camera → hardware H.264 encoder. The camera draws straight into the encoder's input surface, so frames never touch
 * the CPU. Encoded output goes to [sink]. All camera work happens on one background thread.
 */
class CameraStreamer(
    context: Context,
    private val sink: (flags: Int, ptsUs: Long, data: ByteArray, length: Int) -> Unit,
    private val onState: (running: Boolean, actual: StreamSettings?, error: String?) -> Unit,
) {
    private val manager = context.getSystemService(CameraManager::class.java)
    private val thread = HandlerThread("tether-camera").apply { start() }
    private val handler = Handler(thread.looper)

    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    /** The phone-screen preview, when the activity shows one. Part of the capture session alongside the encoder. */
    private var previewSurface: Surface? = null
    private var drain: Thread? = null
    private var current: StreamSettings? = null
    /** Bumped on every (re)start so callbacks from an older camera session are ignored. */
    private var generation = 0

    val isRunning get() = current != null

    fun cameras(): List<Pair<String, String>> = manager.cameraIdList.mapNotNull { id ->
        val c = manager.getCameraCharacteristics(id)
        val facing = when (c.get(CameraCharacteristics.LENS_FACING)) {
            CameraCharacteristics.LENS_FACING_FRONT -> "front"
            CameraCharacteristics.LENS_FACING_BACK -> "back"
            else -> "external"
        }
        id to facing
    }

    fun hasTorch(id: String) = runCatching {
        manager.getCameraCharacteristics(id).get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
    }.getOrDefault(false)

    /** Highest frame rate this camera can hold in a normal session (many phones: 30). */
    fun maxFps(id: String) = runCatching {
        manager.getCameraCharacteristics(id).get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)
            ?.maxOf { it.upper } ?: 30
    }.getOrDefault(30)

    /** A light 16:9 size for the on-phone preview (it only needs to look right on a small screen). */
    fun previewSize(id: String): Size = runCatching {
        val map = manager.getCameraCharacteristics(id).get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        val sizes = map?.getOutputSizes(android.view.SurfaceHolder::class.java)?.toList() ?: emptyList()
        sizes.filter { it.width * 9 == it.height * 16 && it.width <= 1280 }.maxByOrNull { it.width }
            ?: sizes.filter { it.width <= 1280 }.maxByOrNull { it.width * it.height }
    }.getOrNull() ?: Size(1280, 720)

    /**
     * Adds or removes the on-phone preview. Removing blocks briefly until the camera stopped drawing into it, so the
     * screen can turn off (and destroy the surface) without breaking the stream to the PC.
     */
    fun setPreview(target: Surface?) {
        val done = java.util.concurrent.CountDownLatch(1)
        handler.post {
            if (previewSurface !== target) {
                previewSurface = target
                val d = device
                if (d != null) {
                    runCatching { session?.close() }
                    session = null
                    createSession(d, generation)
                }
            }
            done.countDown()
        }
        if (target == null) done.await(800, java.util.concurrent.TimeUnit.MILLISECONDS)
    }

    /** Starts the camera, or adjusts it: a torch-only change keeps the stream running. */
    fun apply(wanted: StreamSettings) = handler.post {
        val cur = current
        if (cur != null && cur.sameStream(wanted) && device != null) {
            // Keep what was resolved for the stream; take the rest (torch, preview, dim) as asked.
            current = wanted.copy(cameraId = cur.cameraId, width = cur.width, height = cur.height, fps = cur.fps)
            repeat()
            onState(true, current, null)
        } else {
            stopNow()
            startNow(wanted)
        }
    }

    fun requestKeyFrame() = handler.post { requestKeyFrameNow() }

    private fun requestKeyFrameNow() {
        codec?.let { c ->
            runCatching { c.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) }) }
        }
    }

    fun stop() = handler.post { stopNow() }

    fun release() {
        handler.post { stopNow(); thread.quitSafely() }
    }

    // ---------- internals (camera thread only) ----------

    @SuppressLint("MissingPermission") // checked by the activity before the service starts
    private fun startNow(wanted: StreamSettings) {
        val gen = ++generation
        val s = resolve(wanted)
        try {
            val (c, input) = createEncoder(s)
            codec = c
            surface = input
            startDrain(c, gen)
        } catch (e: Exception) {
            Log.e(TAG, "encoder", e)
            onState(false, null, "The phone's video encoder refused ${s.width}x${s.height}@${s.fps}")
            return
        }
        current = s
        manager.openCamera(s.cameraId, object : CameraDevice.StateCallback() {
            override fun onOpened(d: CameraDevice) {
                if (gen != generation) { d.close(); return }
                device = d
                createSession(d, gen)
            }

            override fun onDisconnected(d: CameraDevice) {
                d.close()
                if (gen == generation) { stopNow(); onState(false, null, "Another app took the camera") }
            }

            override fun onError(d: CameraDevice, error: Int) {
                d.close()
                if (gen == generation) { stopNow(); onState(false, null, "The camera reported error $error") }
            }
        }, handler)
    }

    @Suppress("DEPRECATION") // SessionConfiguration needs API 28; this works everywhere
    private fun createSession(d: CameraDevice, gen: Int) {
        val target = surface ?: return
        val preview = previewSurface?.takeIf { it.isValid }
        val targets = listOfNotNull(target, preview)
        d.createCaptureSession(targets, object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(s: CameraCaptureSession) {
                if (gen != generation) { s.close(); return }
                session = s
                repeat()
                requestKeyFrameNow()
                onState(true, current, null)
            }

            override fun onConfigureFailed(s: CameraCaptureSession) {
                if (gen != generation) return
                if (preview != null) {
                    // The preview size didn't work with this stream: keep streaming without it.
                    Log.w(TAG, "session with preview failed, retrying without")
                    previewSurface = null
                    createSession(d, gen)
                } else {
                    stopNow(); onState(false, null, "The camera can't stream this format")
                }
            }
        }, handler)
    }

    private fun repeat() {
        val d = device ?: return
        val s = session ?: return
        val cfg = current ?: return
        val target = surface ?: return
        val chars = manager.getCameraCharacteristics(cfg.cameraId)
        val req = d.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
            addTarget(target)
            previewSurface?.takeIf { it.isValid }?.let { addTarget(it) }
            set(CaptureRequest.CONTROL_MODE, CaptureRequest.CONTROL_MODE_AUTO)
            fpsRange(chars, cfg.fps)?.let { set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, it) }
            val af = chars.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES) ?: intArrayOf()
            if (CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO in af) {
                set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
            }
            set(
                CaptureRequest.FLASH_MODE,
                if (cfg.torch && hasTorch(cfg.cameraId)) CaptureRequest.FLASH_MODE_TORCH else CaptureRequest.FLASH_MODE_OFF,
            )
        }
        runCatching { s.setRepeatingRequest(req.build(), null, handler) }
            .onFailure { Log.e(TAG, "repeat", it) }
    }

    private fun stopNow() {
        generation++
        runCatching { session?.close() }
        runCatching { device?.close() }
        session = null
        device = null
        val c = codec
        codec = null
        drain?.interrupt()
        drain = null
        if (c != null) {
            runCatching { c.stop() }
            runCatching { c.release() }
        }
        runCatching { surface?.release() }
        surface = null
        if (current != null) {
            current = null
            onState(false, null, null)
        }
    }

    private fun startDrain(c: MediaCodec, gen: Int) {
        drain = Thread({
            val info = MediaCodec.BufferInfo()
            var buf = ByteArray(256 * 1024)
            while (!Thread.currentThread().isInterrupted && gen == generation) {
                val index = try { c.dequeueOutputBuffer(info, 20_000) } catch (e: IllegalStateException) { break }
                if (index < 0) continue
                try {
                    val out = c.getOutputBuffer(index)
                    if (out != null && info.size > 0) {
                        if (buf.size < info.size) buf = ByteArray(info.size * 2)
                        out.position(info.offset)
                        out.get(buf, 0, info.size)
                        var flags = 0
                        if (info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0) flags = flags or Proto.FLAG_KEYFRAME
                        if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) flags = flags or Proto.FLAG_CONFIG
                        sink(flags, info.presentationTimeUs, buf, info.size)
                    }
                    c.releaseOutputBuffer(index, false)
                } catch (e: IllegalStateException) {
                    break
                }
            }
        }, "tether-encoder").apply { start() }
    }

    /** Picks what the camera and encoder can really do, as close to the request as possible. */
    private fun resolve(w: StreamSettings): StreamSettings {
        val ids = manager.cameraIdList
        val id = if (w.cameraId in ids) w.cameraId else ids.first()
        val chars = manager.getCameraCharacteristics(id)
        val map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        val sizes = map?.getOutputSizes(MediaCodec::class.java)?.toList() ?: emptyList()
        val size = sizes.firstOrNull { it.width == w.width && it.height == w.height }
            ?: sizes.filter { it.width * 9 == it.height * 16 && it.width <= w.width }.maxByOrNull { it.width }
            ?: sizes.filter { it.width <= w.width }.maxByOrNull { it.width * it.height }
            ?: Size(1280, 720)
        val fps = if (fpsRange(chars, w.fps)?.upper == w.fps) w.fps else 30
        return w.copy(cameraId = id, width = size.width, height = size.height, fps = fps)
    }

    private fun fpsRange(chars: CameraCharacteristics, fps: Int): Range<Int>? {
        val ranges = chars.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES) ?: return null
        // Prefer a fixed rate (e.g. 30-30); otherwise the widest range that tops out at the wanted rate.
        return ranges.filter { it.upper == fps }.maxByOrNull { it.lower }
            ?: ranges.filter { it.upper >= fps }.minByOrNull { it.upper }
    }

    private fun createEncoder(s: StreamSettings): Pair<MediaCodec, Surface> {
        fun format(extras: Boolean) = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, s.width, s.height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, s.bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, s.fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
            // Keep sending frames when the picture doesn't change, so the PC never thinks we froze.
            setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 1_000_000L / s.fps * 3)
            if (extras) {
                setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
                if (Build.VERSION.SDK_INT >= 29) {
                    setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
                    setInteger(MediaFormat.KEY_PREPEND_HEADER_TO_SYNC_FRAMES, 1)
                }
                if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LATENCY, 1)
            }
        }
        for (extras in listOf(true, false)) {
            val c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
            try {
                c.configure(format(extras), null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                val input = c.createInputSurface()
                c.start()
                return c to input
            } catch (e: Exception) {
                c.release()
                if (!extras) throw e
                Log.w(TAG, "encoder rejected low-latency keys, retrying plain", e)
            }
        }
        error("unreachable")
    }

    companion object {
        private const val TAG = "TetherCamera"
    }
}
