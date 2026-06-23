package alvr.client.camera;

import android.content.Context;
import android.graphics.ImageFormat;
import android.graphics.Rect;
import android.hardware.camera2.CameraCaptureSession;
import android.hardware.camera2.CameraCharacteristics;
import android.hardware.camera2.CameraDevice;
import android.hardware.camera2.CameraManager;
import android.hardware.camera2.CaptureRequest;
import android.hardware.camera2.params.StreamConfigurationMap;
import android.media.Image;
import android.media.ImageReader;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Log;
import android.util.Size;
import android.view.Surface;

import java.nio.ByteBuffer;
import java.util.Arrays;
import java.util.Collections;

/**
 * Java-side Passthrough Camera helper, driven from Rust via JNI.
 *
 * Compiled to a dex (scripts/build_camera_helper.ps1), embedded in the .so and
 * loaded at runtime via InMemoryDexClassLoader. Camera2's async StateCallback
 * and Meta's Java-only vendor tags require this to live in Java.
 */
public class CameraHelper {
    private static final String TAG = "ALVR-CameraHelper";

    // ---- Camera2 state (single passthrough stream) ----
    private static CameraDevice sDevice;
    private static CameraCaptureSession sSession;
    private static ImageReader sReader;
    private static HandlerThread sBgThread;
    private static Handler sBgHandler;

    private static final Object sFrameLock = new Object();
    private static byte[] sLatestGray; // tightly packed grayscale (Y plane, no padding)
    private static int sW, sH;
    private static long sTimestampNs;
    private static int sFrameCount;

    // Calibration source (set in startPassthrough).
    private static CameraCharacteristics sChars;
    private static int sStreamW, sStreamH;

    public static String ping() {
        Log.i(TAG, "ping");
        return "pong-from-java";
    }

    /** Stage 2a: enumerate cameras + Meta vendor tags. */
    public static String enumerateCameras(Context context) {
        try {
            CameraManager cm =
                (CameraManager) context.getSystemService(Context.CAMERA_SERVICE);
            String[] ids = cm.getCameraIdList();
            StringBuilder sb = new StringBuilder("count=" + ids.length + " ");
            for (String id : ids) {
                CameraCharacteristics cc = cm.getCameraCharacteristics(id);
                Byte src = getVendorByte(cc, "com.meta.extra_metadata.camera_source");
                Byte pos = getVendorByte(cc, "com.meta.extra_metadata.position");
                sb.append("[id=").append(id).append(" source=").append(src)
                  .append(" pos=").append(pos).append("] ");
            }
            String r = sb.toString();
            Log.i(TAG, "enumerateCameras: " + r);
            return r;
        } catch (Throwable t) {
            Log.e(TAG, "enumerateCameras failed", t);
            return "ERROR: " + t;
        }
    }

    /**
     * Stage 2b: open the passthrough LEFT camera (source=0, position=0) and start
     * a YUV stream. Frames arrive on a background thread; the latest grayscale
     * frame is cached. Returns "OK id=.. WxH" or "ERROR: ..".
     */
    public static String startPassthrough(Context context) {
        try {
            CameraManager cm =
                (CameraManager) context.getSystemService(Context.CAMERA_SERVICE);

            String camId = null;
            for (String id : cm.getCameraIdList()) {
                CameraCharacteristics cc = cm.getCameraCharacteristics(id);
                Byte src = getVendorByte(cc, "com.meta.extra_metadata.camera_source");
                Byte pos = getVendorByte(cc, "com.meta.extra_metadata.position");
                if (src != null && src == 0 && pos != null && pos == 0) {
                    camId = id;
                    break;
                }
            }
            if (camId == null) {
                return "ERROR: no passthrough camera (permission not granted?)";
            }

            CameraCharacteristics cc = cm.getCameraCharacteristics(camId);
            StreamConfigurationMap map =
                cc.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP);
            Size[] sizes = map.getOutputSizes(ImageFormat.YUV_420_888);
            // Log every available size (for the resolution-vs-QR-size evaluation).
            StringBuilder sb = new StringBuilder("YUV sizes:");
            for (Size s : sizes) {
                sb.append(' ').append(s.getWidth()).append('x').append(s.getHeight());
            }
            Log.i(TAG, sb.toString());
            // Pick the largest available size: more pixels on the QR => better
            // corner localization (depth/rotation accuracy) and longer range.
            Size chosen = sizes[0];
            for (Size s : sizes) {
                if ((long) s.getWidth() * s.getHeight()
                        > (long) chosen.getWidth() * chosen.getHeight()) {
                    chosen = s;
                }
            }
            final int w = chosen.getWidth();
            final int h = chosen.getHeight();

            // Keep characteristics + stream size for calibration queries.
            sChars = cc;
            sStreamW = w;
            sStreamH = h;

            sBgThread = new HandlerThread("alvr-cam");
            sBgThread.start();
            sBgHandler = new Handler(sBgThread.getLooper());

            sReader = ImageReader.newInstance(w, h, ImageFormat.YUV_420_888, 3);
            sReader.setOnImageAvailableListener(reader -> {
                Image img = reader.acquireLatestImage();
                if (img == null) {
                    return;
                }
                try {
                    Image.Plane yPlane = img.getPlanes()[0];
                    ByteBuffer buf = yPlane.getBuffer();
                    int rowStride = yPlane.getRowStride();
                    int iw = img.getWidth();
                    int ih = img.getHeight();
                    // Copy Y plane into a tightly packed grayscale buffer.
                    byte[] gray = new byte[iw * ih];
                    byte[] row = new byte[rowStride];
                    for (int y = 0; y < ih; y++) {
                        buf.position(y * rowStride);
                        int n = Math.min(rowStride, buf.remaining());
                        buf.get(row, 0, n);
                        System.arraycopy(row, 0, gray, y * iw, iw);
                    }
                    synchronized (sFrameLock) {
                        sLatestGray = gray;
                        sW = iw;
                        sH = ih;
                        sTimestampNs = img.getTimestamp();
                        sFrameCount++;
                    }
                } catch (Throwable t) {
                    Log.e(TAG, "frame copy failed", t);
                } finally {
                    img.close();
                }
            }, sBgHandler);

            final String fcamId = camId;
            cm.openCamera(camId, new CameraDevice.StateCallback() {
                @Override
                public void onOpened(CameraDevice device) {
                    sDevice = device;
                    try {
                        Surface surface = sReader.getSurface();
                        CaptureRequest.Builder rb =
                            device.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW);
                        rb.addTarget(surface);
                        device.createCaptureSession(
                            Collections.singletonList(surface),
                            new CameraCaptureSession.StateCallback() {
                                @Override
                                public void onConfigured(CameraCaptureSession session) {
                                    sSession = session;
                                    try {
                                        session.setRepeatingRequest(rb.build(), null, sBgHandler);
                                        Log.i(TAG, "passthrough streaming started id=" + fcamId);
                                    } catch (Throwable t) {
                                        Log.e(TAG, "setRepeatingRequest failed", t);
                                    }
                                }

                                @Override
                                public void onConfigureFailed(CameraCaptureSession session) {
                                    Log.e(TAG, "capture session configure failed");
                                }
                            },
                            sBgHandler);
                    } catch (Throwable t) {
                        Log.e(TAG, "onOpened failed", t);
                    }
                }

                @Override
                public void onDisconnected(CameraDevice device) {
                    Log.w(TAG, "camera disconnected");
                    device.close();
                    sDevice = null;
                }

                @Override
                public void onError(CameraDevice device, int error) {
                    Log.e(TAG, "camera error " + error);
                    device.close();
                    sDevice = null;
                }
            }, sBgHandler);

            return "OK id=" + camId + " " + w + "x" + h;
        } catch (Throwable t) {
            Log.e(TAG, "startPassthrough failed", t);
            return "ERROR: " + t;
        }
    }

    /** Stage 2b verification: report whether frames are flowing. */
    public static String getFrameInfo() {
        synchronized (sFrameLock) {
            return "frames=" + sFrameCount + " size=" + sW + "x" + sH
                + " bytes=" + (sLatestGray == null ? 0 : sLatestGray.length)
                + " ts=" + sTimestampNs;
        }
    }

    // ---- Stage 2d: frame data accessors for Rust-side QR decode ----

    /** Latest tightly-packed grayscale frame (width*height bytes), or null. */
    public static byte[] getLatestGray() {
        synchronized (sFrameLock) {
            return sLatestGray;
        }
    }

    public static int getFrameWidth() {
        synchronized (sFrameLock) {
            return sW;
        }
    }

    public static int getFrameHeight() {
        synchronized (sFrameLock) {
            return sH;
        }
    }

    /**
     * Stage 2c: camera intrinsics + extrinsics for PnP.
     * Returns a parseable string:
     *   intr=fx,fy,cx,cy,skew  active=W,H  stream=W,H
     *   lensT=x,y,z  lensR=x,y,z,w  dist=k1,k2,...
     * LENS_INTRINSIC_CALIBRATION is relative to the pre-correction active array,
     * so Rust must scale (fx,cx by streamW/activeW; fy,cy by streamH/activeH).
     */
    public static String getCalibration() {
        try {
            if (sChars == null) {
                return "ERROR: not started";
            }
            float[] intr = sChars.get(CameraCharacteristics.LENS_INTRINSIC_CALIBRATION);
            Rect active =
                sChars.get(CameraCharacteristics.SENSOR_INFO_PRE_CORRECTION_ACTIVE_ARRAY_SIZE);
            float[] lensT = sChars.get(CameraCharacteristics.LENS_POSE_TRANSLATION);
            float[] lensR = sChars.get(CameraCharacteristics.LENS_POSE_ROTATION);
            float[] dist = sChars.get(CameraCharacteristics.LENS_DISTORTION);

            int aw = active == null ? 0 : active.width();
            int ah = active == null ? 0 : active.height();

            String r = "intr=" + fmt(intr)
                + " active=" + aw + "," + ah
                + " stream=" + sStreamW + "," + sStreamH
                + " lensT=" + fmt(lensT)
                + " lensR=" + fmt(lensR)
                + " dist=" + fmt(dist);
            Log.i(TAG, "getCalibration: " + r);
            return r;
        } catch (Throwable t) {
            Log.e(TAG, "getCalibration failed", t);
            return "ERROR: " + t;
        }
    }

    private static String fmt(float[] a) {
        if (a == null) {
            return "null";
        }
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < a.length; i++) {
            if (i > 0) {
                sb.append(",");
            }
            sb.append(a[i]);
        }
        return sb.toString();
    }

    private static Byte getVendorByte(CameraCharacteristics cc, String name) {
        try {
            CameraCharacteristics.Key<Byte> key =
                new CameraCharacteristics.Key<>(name, Byte.class);
            return cc.get(key);
        } catch (Throwable t) {
            return null;
        }
    }
}
