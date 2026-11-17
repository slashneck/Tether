package io.github.slashneck.tether

import android.os.Handler
import android.os.Looper

/** What the screen shows. The service publishes, the activity listens. Everything runs on the main thread. */
data class TetherState(
    val serviceOn: Boolean = false,
    val connected: Boolean = false,
    val pcName: String = "",
    val streaming: Boolean = false,
    val settings: StreamSettings = StreamSettings(),
    val error: String = "",
    /** When the PC app quit and asked the phone to shut down too (uptime ms, 0 = never). */
    val closedByPcAt: Long = 0,
)

object Bus {
    private val main = Handler(Looper.getMainLooper())
    private val listeners = mutableListOf<(TetherState) -> Unit>()

    @Volatile
    var state = TetherState()
        private set

    fun update(change: (TetherState) -> TetherState) {
        main.post {
            state = change(state)
            listeners.toList().forEach { it(state) }
        }
    }

    fun listen(listener: (TetherState) -> Unit) {
        listeners += listener
        listener(state)
    }

    fun unlisten(listener: (TetherState) -> Unit) {
        listeners -= listener
    }
}
