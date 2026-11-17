package io.github.slashneck.tether

import org.json.JSONObject
import java.io.DataInputStream
import java.io.IOException
import java.io.OutputStream
import java.net.Socket
import java.net.SocketTimeoutException

/** Wire protocol v1, see docs/PROTOCOL.md. */
object Proto {
    const val VERSION = 1
    const val PORT = 27183

    const val HELLO = 0x01
    const val VIDEO = 0x02
    const val STATUS = 0x03
    const val PONG = 0x11

    const val SETTINGS = 0x81
    const val KEYFRAME = 0x82
    const val BYE = 0x83
    const val PING = 0x90

    const val FLAG_KEYFRAME = 1
    const val FLAG_CONFIG = 2

    /** No message from the PC for this long means the link is dead (the PC pings every 500 ms). */
    const val TIMEOUT_MS = 3000
}

data class StreamSettings(
    val cameraId: String = "0",
    val width: Int = 1920,
    val height: Int = 1080,
    val fps: Int = 30,
    val bitrate: Int = 8_000_000,
    val torch: Boolean = false,
    /** Show the camera on the phone's screen while streaming. Off saves battery. */
    val preview: Boolean = true,
    /** Dim the phone's screen to minimum after a few seconds without touches. */
    val dim: Boolean = true,
) {
    fun merge(json: JSONObject) = StreamSettings(
        cameraId = json.optString("camera", cameraId),
        width = json.optInt("width", width),
        height = json.optInt("height", height),
        fps = json.optInt("fps", fps),
        bitrate = json.optInt("bitrate", bitrate),
        torch = json.optBoolean("torch", torch),
        preview = json.optBoolean("preview", preview),
        dim = json.optBoolean("dim", dim),
    )

    /** Same camera stream, only the torch differs: no need to restart the camera. */
    fun sameStream(o: StreamSettings) =
        cameraId == o.cameraId && width == o.width && height == o.height && fps == o.fps && bitrate == o.bitrate
}

/** One connection to the PC. Writes are thread-safe; reading happens on the caller's thread. */
class Link(private val socket: Socket) {
    private val input = DataInputStream(socket.getInputStream().buffered())
    private val output: OutputStream = socket.getOutputStream()
    private val writeLock = Any()

    init {
        socket.tcpNoDelay = true
        socket.soTimeout = Proto.TIMEOUT_MS
        socket.sendBufferSize = 1 shl 20
    }

    @Volatile
    var closed = false
        private set

    fun send(type: Int, payload: ByteArray, offset: Int = 0, length: Int = payload.size) {
        send(type, null, payload, offset, length)
    }

    /** Sends `head` and `payload` as one message (avoids copying video data into a new array). */
    fun send(type: Int, head: ByteArray?, payload: ByteArray, offset: Int, length: Int) {
        val total = (head?.size ?: 0) + length
        val header = byteArrayOf(
            type.toByte(),
            (total ushr 24).toByte(), (total ushr 16).toByte(), (total ushr 8).toByte(), total.toByte(),
        )
        synchronized(writeLock) {
            if (closed) return
            try {
                output.write(header)
                if (head != null) output.write(head)
                output.write(payload, offset, length)
                output.flush()
            } catch (e: IOException) {
                close()
            }
        }
    }

    fun sendJson(type: Int, json: JSONObject) = send(type, json.toString().toByteArray())

    fun sendVideo(flags: Int, ptsUs: Long, data: ByteArray, offset: Int, length: Int) {
        val head = ByteArray(9)
        head[0] = flags.toByte()
        for (i in 0 until 8) head[1 + i] = (ptsUs ushr (56 - 8 * i)).toByte()
        send(Proto.VIDEO, head, data, offset, length)
    }

    /** Blocks for the next message. Returns null when the link is closed or timed out. */
    fun read(): Pair<Int, ByteArray>? {
        return try {
            val type = input.readUnsignedByte()
            val len = input.readInt()
            if (len < 0 || len > (16 shl 20)) throw IOException("bad length $len")
            val payload = ByteArray(len)
            input.readFully(payload)
            type to payload
        } catch (e: SocketTimeoutException) {
            close(); null
        } catch (e: IOException) {
            close(); null
        }
    }

    fun close() {
        closed = true
        try { socket.close() } catch (_: IOException) {}
    }
}
