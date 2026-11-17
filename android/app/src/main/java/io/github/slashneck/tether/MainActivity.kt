package io.github.slashneck.tether

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.TypedValue
import android.view.Gravity
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.FrameLayout
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Two screens in one:
 *  - Home (no PC): where to plug in, one big button.
 *  - Live (PC connected): the camera preview, and the buttons in the bottom-LEFT corner.
 * While live the screen stays on (so Android never suspends anything) and dims to minimum after a few idle seconds.
 */
class MainActivity : Activity(), SurfaceHolder.Callback {
    private val main = Handler(Looper.getMainLooper())

    // home
    private lateinit var home: View
    private lateinit var headline: TextView
    private lateinit var subline: TextView
    private lateinit var dot: View
    private lateinit var button: Button
    private lateinit var errorText: TextView

    // live
    private lateinit var live: FrameLayout
    private lateinit var previewView: SurfaceView
    private lateinit var previewOff: TextView
    private lateinit var pill: TextView
    private lateinit var previewButton: Button
    private lateinit var lightButton: Button
    private lateinit var dimButton: Button
    private lateinit var dimOverlay: View

    private val createdAt = android.os.SystemClock.uptimeMillis()
    private var dimmed = false
    private var surfaceReady = false
    private var expectedSize: android.util.Size? = null
    private val listener: (TetherState) -> Unit = ::render
    private val dimLater = Runnable { setDimmed(true) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(buildUi())
        // Opening Tether turns it on: plug in, open the app (or let the PC open it), done.
        if (hasCameraPermission()) StreamService.start(this) else askPermissions()
    }

    /** Opened again while already open (e.g. by the PC after a replug): make sure the service runs. */
    override fun onNewIntent(intent: android.content.Intent?) {
        super.onNewIntent(intent)
        if (hasCameraPermission() && !Bus.state.serviceOn) StreamService.start(this)
    }

    override fun onStart() {
        super.onStart()
        Bus.listen(listener)
    }

    override fun onStop() {
        Bus.unlisten(listener)
        main.removeCallbacks(dimLater)
        super.onStop()
    }

    override fun dispatchTouchEvent(ev: MotionEvent): Boolean {
        if (ev.action == MotionEvent.ACTION_DOWN && dimmed) {
            setDimmed(false)
            return true // the waking tap only wakes
        }
        scheduleDim()
        return super.dispatchTouchEvent(ev)
    }

    private fun hasCameraPermission() = checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

    private fun askPermissions() {
        val wanted = mutableListOf(Manifest.permission.CAMERA)
        if (Build.VERSION.SDK_INT >= 33) wanted += Manifest.permission.POST_NOTIFICATIONS
        requestPermissions(wanted.toTypedArray(), 1)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        if (hasCameraPermission()) StreamService.start(this) else render(Bus.state)
    }

    private fun onButton() {
        when {
            !hasCameraPermission() -> askPermissions()
            Bus.state.serviceOn -> StreamService.stop(this)
            else -> StreamService.start(this)
        }
    }

    // ---------- state → screen ----------

    private fun render(s: TetherState) {
        // The PC app quit just now: close too. (Only a fresh quit; reopening Tether later works normally.)
        if (s.closedByPcAt > createdAt && android.os.SystemClock.uptimeMillis() - s.closedByPcAt < 5000) {
            finishAndRemoveTask()
            return
        }
        val isLive = s.connected
        home.visibility = if (isLive) View.GONE else View.VISIBLE
        live.visibility = if (isLive) View.VISIBLE else View.GONE

        if (isLive) {
            window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        } else {
            window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            if (dimmed) setDimmed(false)
        }
        scheduleDim()

        // home
        headline.text = when {
            !hasCameraPermission() -> "Tether needs the camera"
            !s.serviceOn -> "Tether is off"
            else -> "Waiting for your PC"
        }
        subline.text = when {
            !hasCameraPermission() -> "Allow camera access to stream to your PC"
            !s.serviceOn -> "Turn it on to stream to your PC"
            else -> "Plug into your PC and open Tether there"
        }
        (dot.background as GradientDrawable).setColor(if (s.serviceOn) WHITE else GREY_DARK)
        button.text = if (s.serviceOn) "Stop" else "Start"
        errorText.text = s.error
        errorText.visibility = if (s.error.isEmpty()) View.GONE else View.VISIBLE

        // live
        val st = s.settings
        pill.text = if (s.streaming) "LIVE · USB · ${st.height}p${st.fps}" else "Starting camera…"
        previewButton.text = if (st.preview) "Preview\non" else "Preview\noff"
        lightButton.text = if (st.torch) "Light\non" else "Light\noff"
        dimButton.text = if (st.dim) "Auto-dim\non" else "Auto-dim\noff"
        styleToggle(previewButton, st.preview)
        styleToggle(lightButton, st.torch)
        styleToggle(dimButton, st.dim)
        updatePreview()
    }

    /** The preview is shown only while live, switched on, and not dimmed (dimmed = nobody's looking). */
    private fun previewWanted(): Boolean {
        val s = Bus.state
        return s.connected && s.streaming && s.settings.preview && !dimmed
    }

    private fun updatePreview() {
        val want = previewWanted()
        previewOff.visibility = if (want) View.GONE else View.VISIBLE
        previewOff.text = if (Bus.state.settings.preview) "" else "Preview off\nsaving battery"
        if (want && previewView.visibility != View.VISIBLE) {
            val size = StreamService.instance?.previewSize() ?: return
            expectedSize = size
            previewView.holder.setFixedSize(size.width, size.height)
            previewView.visibility = View.VISIBLE
        } else if (!want && previewView.visibility == View.VISIBLE) {
            StreamService.instance?.setPreview(null)
            previewView.visibility = View.GONE // destroys the surface
        }
        layoutPreview()
    }

    override fun surfaceCreated(holder: SurfaceHolder) {}

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        val e = expectedSize ?: return
        if (width == e.width && height == e.height && previewWanted()) {
            surfaceReady = true
            StreamService.instance?.setPreview(holder.surface)
        }
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        // Must detach before returning, or the camera keeps drawing into a dead surface and the stream stalls.
        if (surfaceReady) StreamService.instance?.setPreview(null)
        surfaceReady = false
    }

    /** Fits the (portrait, 9:16) preview inside the screen. */
    private fun layoutPreview() {
        val w = live.width
        val h = live.height
        if (w == 0 || h == 0) return
        val e = expectedSize ?: return
        // The camera framework rotates the picture upright for portrait, so the shown aspect is height:width.
        val (aw, ah) = e.height to e.width
        val (vw, vh) = if (w * ah > h * aw) (h * aw / ah) to h else w to (w * ah / aw)
        val lp = previewView.layoutParams as FrameLayout.LayoutParams
        if (lp.width != vw || lp.height != vh) {
            lp.width = vw
            lp.height = vh
            lp.gravity = Gravity.CENTER
            previewView.layoutParams = lp
        }
    }

    // ---------- dimming ----------

    private fun scheduleDim() {
        main.removeCallbacks(dimLater)
        val s = Bus.state
        if (s.connected && s.settings.dim && !dimmed) main.postDelayed(dimLater, DIM_AFTER_MS)
    }

    private fun setDimmed(on: Boolean) {
        if (dimmed == on) return
        dimmed = on
        dimOverlay.visibility = if (on) View.VISIBLE else View.GONE
        window.attributes = window.attributes.apply {
            screenBrightness = if (on) 0.01f else WindowManager.LayoutParams.BRIGHTNESS_OVERRIDE_NONE
        }
        updatePreview()
        scheduleDim()
    }

    // ---------- layout (code, no XML) ----------

    private fun dp(v: Int) = TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, v.toFloat(), resources.displayMetrics).toInt()

    private fun text(size: Float, color: Int, bold: Boolean = false) = TextView(this).apply {
        setTextSize(TypedValue.COMPLEX_UNIT_SP, size)
        setTextColor(color)
        if (bold) typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    }

    private fun roundButton(sizeDp: Int, onClick: () -> Unit) = Button(this).apply {
        setTextSize(TypedValue.COMPLEX_UNIT_SP, 11f)
        isAllCaps = false
        stateListAnimator = null
        setPadding(0, 0, 0, 0)
        minWidth = 0
        minHeight = 0
        layoutParams = LinearLayout.LayoutParams(dp(sizeDp), dp(sizeDp)).apply { topMargin = dp(12) }
        setOnClickListener { onClick() }
    }

    private fun styleToggle(b: Button, on: Boolean) {
        b.background = GradientDrawable().apply {
            shape = GradientDrawable.OVAL
            if (on) setColor(WHITE) else { setColor(BLACK); setStroke(dp(1), LINE_STRONG) }
        }
        b.setTextColor(if (on) BLACK else WHITE)
    }

    private fun buildUi(): View {
        val root = FrameLayout(this).apply { setBackgroundColor(BLACK) }
        home = buildHome()
        root.addView(home, FrameLayout.LayoutParams(-1, -1))
        live = buildLive()
        root.addView(live, FrameLayout.LayoutParams(-1, -1))
        dimOverlay = FrameLayout(this).apply {
            setBackgroundColor(Color.BLACK)
            visibility = View.GONE
            addView(text(13f, GREY_DARK).apply {
                text = "Streaming · tap to wake"
            }, FrameLayout.LayoutParams(-2, -2, Gravity.BOTTOM or Gravity.START).apply {
                leftMargin = dp(24); bottomMargin = dp(48)
            })
        }
        root.addView(dimOverlay, FrameLayout.LayoutParams(-1, -1))

        // Edge to edge: keep content clear of the status and navigation bars.
        root.setOnApplyWindowInsetsListener { _, insets ->
            @Suppress("DEPRECATION")
            val top = insets.systemWindowInsetTop
            @Suppress("DEPRECATION")
            val bottom = insets.systemWindowInsetBottom
            home.setPadding(dp(24), top + dp(24), dp(24), bottom + dp(24))
            live.setPadding(0, top, 0, bottom)
            insets
        }
        live.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> layoutPreview() }
        return root
    }

    private fun buildHome(): View {
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }

        val top = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        top.addView(ImageView(this).apply { setImageResource(R.drawable.ic_launcher) }, LinearLayout.LayoutParams(dp(32), dp(32)))
        top.addView(text(18f, WHITE, bold = true).apply { text = "Tether"; setPadding(dp(10), 0, 0, 0) })
        root.addView(top)

        headline = text(30f, WHITE, bold = true).apply { setPadding(0, dp(28), 0, dp(8)); letterSpacing = -0.02f }
        root.addView(headline)

        val status = LinearLayout(this).apply { gravity = Gravity.CENTER_VERTICAL }
        dot = View(this).apply { background = GradientDrawable().apply { shape = GradientDrawable.OVAL; setColor(GREY_DARK) } }
        status.addView(dot, LinearLayout.LayoutParams(dp(8), dp(8)).apply { marginEnd = dp(8) })
        subline = text(14f, GREY)
        status.addView(subline)
        root.addView(status)

        errorText = text(13f, WHITE).apply {
            setPadding(dp(12), dp(10), dp(12), dp(10))
            background = GradientDrawable().apply { setStroke(dp(1), WHITE); cornerRadius = dp(8).toFloat() }
            visibility = View.GONE
        }
        root.addView(errorText, LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(16) })

        val middle = LinearLayout(this).apply { gravity = Gravity.CENTER }
        button = Button(this).apply {
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 18f)
            setTextColor(BLACK)
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
            isAllCaps = false
            stateListAnimator = null
            background = GradientDrawable().apply { shape = GradientDrawable.OVAL; setColor(WHITE) }
            setOnClickListener { onButton() }
        }
        middle.addView(button, LinearLayout.LayoutParams(dp(170), dp(170)))
        root.addView(middle, LinearLayout.LayoutParams(-1, 0, 1f))

        root.addView(text(13f, GREY).apply {
            text = "Camera, quality and the rest are set in the Tether app on your PC."
        })
        return root
    }

    private fun buildLive(): FrameLayout {
        val root = FrameLayout(this)
        previewView = SurfaceView(this).apply { visibility = View.GONE; holder.addCallback(this@MainActivity) }
        root.addView(previewView, FrameLayout.LayoutParams(-1, -1, Gravity.CENTER))

        previewOff = text(15f, GREY, bold = true).apply { gravity = Gravity.START }
        root.addView(previewOff, FrameLayout.LayoutParams(-2, -2, Gravity.CENTER_VERTICAL or Gravity.START).apply {
            leftMargin = dp(24)
        })

        pill = text(12f, WHITE, bold = true).apply {
            setPadding(dp(12), dp(7), dp(12), dp(7))
            background = GradientDrawable().apply { setColor(BLACK); setStroke(dp(1), LINE_STRONG); cornerRadius = dp(99).toFloat() }
        }
        root.addView(pill, FrameLayout.LayoutParams(-2, -2, Gravity.TOP or Gravity.START).apply {
            leftMargin = dp(16); topMargin = dp(16)
        })

        // Bottom-left column of buttons.
        val buttons = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        previewButton = roundButton(64) {
            StreamService.instance?.changeFromPhone { it.copy(preview = !it.preview) }
        }
        lightButton = roundButton(64) {
            StreamService.instance?.changeFromPhone { it.copy(torch = !it.torch) }
        }
        dimButton = roundButton(64) {
            StreamService.instance?.changeFromPhone { it.copy(dim = !it.dim) }
        }
        val dimNow = roundButton(64) { setDimmed(true) }.apply { text = "Dim\nnow" }
        styleToggle(dimNow, false)
        listOf(previewButton, lightButton, dimButton, dimNow).forEach { buttons.addView(it) }
        root.addView(buttons, FrameLayout.LayoutParams(-2, -2, Gravity.BOTTOM or Gravity.START).apply {
            leftMargin = dp(16); bottomMargin = dp(24)
        })
        return root
    }

    companion object {
        private const val DIM_AFTER_MS = 15_000L
        private val BLACK = Color.parseColor("#0A0A0A")
        private val WHITE = Color.parseColor("#FAFAFA")
        private val GREY = Color.parseColor("#A3A3A3")
        private val GREY_DARK = Color.parseColor("#737373")
        private val LINE_STRONG = Color.parseColor("#404040")
    }
}
