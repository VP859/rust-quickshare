package io.github.vp859.ferry

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat

/**
 * A question to the user – an offered file, a batch, a text – as a
 * notification with "Accept" and "Decline", answered without opening Ferry.
 *
 * Rust shows and removes it ([show], [dismiss], through JNI, see
 * `platform.rs`); a button reaches [OfferActionReceiver], which hands the
 * answer back to Rust through the native [answer]. Tapping the notification
 * itself opens Ferry with the question, as before. Pairing requests never get
 * buttons: their code has to be compared on both screens.
 */
object OfferActions {
  private const val TAG = "FerryOfferActions"
  private const val CHANNEL_ID = "ferry_questions"
  /** One notification per question: the question's ID is the tag. */
  private const val NOTIFICATION_ID = 7
  const val EXTRA_ID = "io.github.vp859.ferry.QUESTION_ID"
  const val EXTRA_ACCEPT = "io.github.vp859.ferry.ACCEPT"

  /** Answers the question [id] in Rust (`answer_from_notification`). */
  @JvmStatic
  external fun answer(id: String, accept: Boolean)

  /**
   * Shows the question [id] with the buttons [acceptLabel] and [declineLabel];
   * returns false when notifications are not allowed (Rust then keeps the
   * question in the app only).
   */
  @JvmStatic
  fun show(
    context: Context,
    id: String,
    title: String,
    text: String,
    acceptLabel: String,
    declineLabel: String,
  ): Boolean {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
      ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) !=
      PackageManager.PERMISSION_GRANTED
    ) {
      return false
    }
    createChannel(context)
    val notification = NotificationCompat.Builder(context, CHANNEL_ID)
      .setSmallIcon(R.drawable.ic_stat_ferry)
      .setContentTitle(title)
      .setContentText(text)
      .setStyle(NotificationCompat.BigTextStyle().bigText(text))
      .setCategory(NotificationCompat.CATEGORY_MESSAGE)
      .setPriority(NotificationCompat.PRIORITY_HIGH)
      .setAutoCancel(true)
      .setContentIntent(openApp(context))
      .addAction(0, declineLabel, answerIntent(context, id, false))
      .addAction(0, acceptLabel, answerIntent(context, id, true))
      .build()
    return try {
      NotificationManagerCompat.from(context).notify(id, NOTIFICATION_ID, notification)
      true
    } catch (e: SecurityException) {
      Log.w(TAG, "Could not show the question", e)
      false
    }
  }

  /** Removes the notification of the question [id] (answered elsewhere, or expired). */
  @JvmStatic
  fun dismiss(context: Context, id: String) {
    NotificationManagerCompat.from(context).cancel(id, NOTIFICATION_ID)
  }

  private fun answerIntent(context: Context, id: String, accept: Boolean): PendingIntent {
    val intent = Intent(context, OfferActionReceiver::class.java)
      .putExtra(EXTRA_ID, id)
      .putExtra(EXTRA_ACCEPT, accept)
    // A request code per question and button, so the extras of one never
    // replace those of another.
    val code = (id.hashCode() * 2) + if (accept) 1 else 0
    return PendingIntent.getBroadcast(
      context, code, intent,
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
  }

  private fun openApp(context: Context) = PendingIntent.getActivity(
    context, 0,
    Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
  )

  private fun createChannel(context: Context) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val channel = NotificationChannel(
      CHANNEL_ID,
      context.getString(R.string.notif_channel_questions),
      NotificationManager.IMPORTANCE_HIGH,
    ).apply { description = context.getString(R.string.notif_channel_questions_desc) }
    context.getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
  }
}

/** The "Accept" and "Decline" buttons of an [OfferActions] notification. */
class OfferActionReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    val id = intent.getStringExtra(OfferActions.EXTRA_ID) ?: return
    val accept = intent.getBooleanExtra(OfferActions.EXTRA_ACCEPT, false)
    OfferActions.dismiss(context, id)
    try {
      OfferActions.answer(id, accept)
    } catch (e: UnsatisfiedLinkError) {
      // The process was restarted since: the question no longer exists.
      Log.w("FerryOfferActions", "No running Ferry for the answer", e)
    }
  }
}
