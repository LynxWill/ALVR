#pragma once

#include "CoreMinimal.h"
#include "Subsystems/GameInstanceSubsystem.h"
#include "HAL/Runnable.h"
#include "HAL/RunnableThread.h"
#include "AnchorTransformData.h"
#include "AnchorReceiverSubsystem.generated.h"

/**
 * Fired on the Game Thread for every response to RequestAnchor.
 *   bIsValid == true  → anchor located, Transform is usable.
 *   bIsValid == false → Quest connected but anchor not found yet.
 */
DECLARE_DYNAMIC_MULTICAST_DELEGATE_OneParam(FOnAnchorReceived, const FAnchorTransformData&, AnchorData);

/**
 * Fired on the Game Thread when RequestAnchor gets no response within
 * the TimeoutSeconds passed to RequestAnchor.  Indicates Quest is unreachable
 * or the app is not running.
 */
DECLARE_DYNAMIC_MULTICAST_DELEGATE(FOnAnchorRequestTimeout);

/**
 * UAnchorReceiverSubsystem
 *
 * Pull-model anchor receiver for the ALVR Quest client.
 * Quest caches anchor at startup and runs a UDP responder on port 9945.
 * UE calls RequestAnchor(QuestIP) whenever it needs fresh data.
 *
 * Three outcomes after RequestAnchor:
 *   1. OnAnchorReceived  (bIsValid=true)  — anchor ready, Transform usable
 *   2. OnAnchorReceived  (bIsValid=false) — Quest connected, anchor not found yet
 *   3. OnAnchorRequestTimeout             — no response (Quest unreachable / not running)
 */
UCLASS(DisplayName = "Quest Anchor Receiver")
class QUESTANCHORRECEIVE_API UAnchorReceiverSubsystem
    : public UGameInstanceSubsystem
    , public FRunnable
{
    GENERATED_BODY()

public:
    // -----------------------------------------------------------------------
    // USubsystem interface
    // -----------------------------------------------------------------------
    virtual void Initialize(FSubsystemCollectionBase& Collection) override;
    virtual void Deinitialize() override;

    // -----------------------------------------------------------------------
    // Blueprint API — Delegates
    // -----------------------------------------------------------------------

    /**
     * Fired for every response from RequestAnchor.
     * Check AnchorData.bIsValid in Blueprint to distinguish ready vs not_found.
     */
    UPROPERTY(BlueprintAssignable, Category = "QuestAnchor")
    FOnAnchorReceived OnAnchorReceived;

    /**
     * Fired when RequestAnchor receives no reply within its TimeoutSeconds.
     * Indicates Quest is unreachable or the ALVR app is not running.
     */
    UPROPERTY(BlueprintAssignable, Category = "QuestAnchor")
    FOnAnchorRequestTimeout OnAnchorRequestTimeout;

    // -----------------------------------------------------------------------
    // Blueprint API — Functions
    // -----------------------------------------------------------------------

    /**
     * Request the latest anchor data from the Quest headset.
     *
     * Sends a one-byte UDP query to QuestIPAddress:QuestPort.
     * Quest responds with current anchor JSON.  Results arrive asynchronously
     * via OnAnchorReceived or OnAnchorRequestTimeout.
     *
     * @param QuestIPAddress  IP address of the Quest (e.g. "192.168.2.171").
     * @param TimeoutSeconds  How long to wait before OnAnchorRequestTimeout fires.
     */
    UFUNCTION(BlueprintCallable, Category = "QuestAnchor")
    void RequestAnchor(const FString& QuestIPAddress, float TimeoutSeconds = 3.0f);

    /** Returns the most recent valid anchor data (or default/invalid if none received). */
    UFUNCTION(BlueprintCallable, Category = "QuestAnchor")
    FAnchorTransformData GetLastAnchorTransform() const;

    /** True after at least one valid anchor packet has been received. */
    UFUNCTION(BlueprintCallable, Category = "QuestAnchor")
    bool IsAnchorValid() const;

    /** Quest's responder port that queries are sent to (the local socket binds an
     *  ephemeral port, so this is the destination port, not a local listen port). */
    UFUNCTION(BlueprintCallable, Category = "QuestAnchor")
    int32 GetQuestPort() const { return QuestPort; }

    /** Change the Quest target port. Read at send time, so it takes effect on the next RequestAnchor call. */
    UFUNCTION(BlueprintCallable, Category = "QuestAnchor")
    void SetQuestPort(int32 NewPort) { QuestPort = NewPort; }

private:
    // -----------------------------------------------------------------------
    // FRunnable — background receive thread
    // -----------------------------------------------------------------------
    virtual bool Init() override;
    virtual uint32 Run() override;
    virtual void Stop() override;

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /** Parse JSON response packet. Returns true for status=="ready", false for not_found. */
    bool ParsePacket(const FString& JsonStr, FAnchorTransformData& OutData) const;

    /**
     * Cancel the pending timeout timer (called from any thread via AsyncTask).
     * Safe to call even when no timer is active.
     */
    void CancelTimeoutTimer();

    /**
     * Convert OpenXR STAGE pose (right-handed, Y-up, metres) to UE transform
     * (left-handed, Z-up, centimetres).
     * ⚠ Validate rotation direction against a physical reference on first run.
     */
    static FTransform ConvertOpenXRToUE(float px, float py, float pz,
                                         float qx, float qy, float qz, float qw);

    // -----------------------------------------------------------------------
    // State
    // -----------------------------------------------------------------------
    class FSocket* ListenSocket = nullptr;
    FRunnableThread* ReceiverThread = nullptr;

    FThreadSafeBool bShouldRun{ false };

    mutable FCriticalSection DataLock;
    FAnchorTransformData LastAnchorData;

    // Quest responder (destination) port. NOTE: 9944 is ALVR's built-in
    // stream_port — Quest uses 9945 to avoid collision. The local UE socket
    // binds an ephemeral port; this is only the target we send queries to.
    int32 QuestPort = 9945;

    /** Timer handle for request timeout. Accessed on Game Thread only. */
    FTimerHandle RequestTimeoutHandle;


    /**
     * Set to true while a RequestAnchor is in flight.
     * Used by the receive thread to know whether to cancel the timeout timer.
     */
    FThreadSafeBool bPendingRequest{ false };
};
