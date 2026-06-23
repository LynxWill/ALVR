#include "AnchorReceiverSubsystem.h"

#include "Dom/JsonObject.h"
#include "Serialization/JsonReader.h"
#include "Serialization/JsonSerializer.h"
#include "Sockets.h"
#include "SocketSubsystem.h"
#include "IPAddress.h"

DEFINE_LOG_CATEGORY_STATIC(LogQuestAnchor, Log, All);

// ---------------------------------------------------------------------------
// USubsystem lifecycle
// ---------------------------------------------------------------------------

void UAnchorReceiverSubsystem::Initialize(FSubsystemCollectionBase& Collection)
{
    Super::Initialize(Collection);

    ISocketSubsystem* SocketSubsystem = ISocketSubsystem::Get(PLATFORM_SOCKETSUBSYSTEM);
    if (!SocketSubsystem)
    {
        UE_LOG(LogQuestAnchor, Error, TEXT("SocketSubsystem not available"));
        return;
    }

    // Create a UDP socket (datagram, non-blocking not needed — the thread uses HasPendingData)
    ListenSocket = SocketSubsystem->CreateSocket(NAME_DGram, TEXT("QuestAnchorReceiver"), false);
    if (!ListenSocket)
    {
        UE_LOG(LogQuestAnchor, Error, TEXT("Failed to create UDP socket"));
        return;
    }

    // Allow address reuse so re-entering PIE doesn't fail
    ListenSocket->SetReuseAddr(true);
    ListenSocket->SetRecvErr(true);

    // Bind to an EPHEMERAL local port (port 0 → OS picks), NOT the fixed Quest
    // port. This is a pull/request-reply client: it sends a query to Quest:9945
    // and Quest replies to whatever source port we sent from, so a fixed local
    // port is unnecessary. Binding a fixed port breaks networked play: in
    // "Play As Client" UE creates multiple GameInstances (server world + client
    // world) in one process, each instantiating this subsystem. Two sockets
    // bound to the same UDP port (even with SO_REUSEADDR) means the OS delivers
    // Quest's reply to only one of them — often not the instance that asked —
    // so the requester times out. Ephemeral ports give each instance a unique
    // source port, eliminating the conflict.
    TSharedRef<FInternetAddr> BindAddr = SocketSubsystem->CreateInternetAddr();
    BindAddr->SetAnyAddress();
    BindAddr->SetPort(0);

    if (!ListenSocket->Bind(*BindAddr))
    {
        UE_LOG(LogQuestAnchor, Error, TEXT("Failed to bind UDP socket to ephemeral port"));
        SocketSubsystem->DestroySocket(ListenSocket);
        ListenSocket = nullptr;
        return;
    }

    UE_LOG(LogQuestAnchor, Log, TEXT("[%p] Subsystem Initialize — local UDP port %d, queries Quest:%d"),
           this, ListenSocket->GetPortNo(), QuestPort);

    // Start receive thread
    bShouldRun = true;
    ReceiverThread = FRunnableThread::Create(this, TEXT("QuestAnchorReceiver"), 0,
                                             TPri_BelowNormal);
}

void UAnchorReceiverSubsystem::Deinitialize()
{
    // Signal thread to stop
    bShouldRun = false;

    if (ReceiverThread)
    {
        ReceiverThread->WaitForCompletion();
        delete ReceiverThread;
        ReceiverThread = nullptr;
    }

    if (ListenSocket)
    {
        ISocketSubsystem* SocketSubsystem = ISocketSubsystem::Get(PLATFORM_SOCKETSUBSYSTEM);
        if (SocketSubsystem)
        {
            SocketSubsystem->DestroySocket(ListenSocket);
        }
        ListenSocket = nullptr;
    }

    UE_LOG(LogQuestAnchor, Log, TEXT("Subsystem shutdown complete"));

    Super::Deinitialize();
}

// ---------------------------------------------------------------------------
// FRunnable — background receive loop
// ---------------------------------------------------------------------------

bool UAnchorReceiverSubsystem::Init()
{
    return ListenSocket != nullptr;
}

uint32 UAnchorReceiverSubsystem::Run()
{
    constexpr int32 BufferSize = 8192;
    TArray<uint8> Buffer;
    Buffer.SetNumUninitialized(BufferSize);

    ISocketSubsystem* SocketSubsystem = ISocketSubsystem::Get(PLATFORM_SOCKETSUBSYSTEM);
    TSharedRef<FInternetAddr> SenderAddr = SocketSubsystem->CreateInternetAddr();

    while (bShouldRun)
    {
        // Poll: check for pending data every 10 ms to stay responsive to Stop()
        uint32 PendingSize = 0;
        if (!ListenSocket->HasPendingData(PendingSize))
        {
            FPlatformProcess::Sleep(0.01f);
            continue;
        }

        int32 BytesRead = 0;
        if (!ListenSocket->RecvFrom(Buffer.GetData(), Buffer.Num(), BytesRead, *SenderAddr)
            || BytesRead <= 0)
        {
            continue;
        }

        // Dedup: only the FIRST packet per RequestAnchor is processed.
        // Duplicates arrive when a proxy/accelerator TUN adapter (Clash, UU, etc.)
        // delivers the same UDP datagram on multiple interfaces to a 0.0.0.0 socket.
        // Atomically claim the pending request so a duplicate can't slip through.
        if (!bPendingRequest.AtomicSet(false))
        {
            UE_LOG(LogQuestAnchor, Verbose,
                   TEXT("[%p] Ignored duplicate/unsolicited packet from %s"),
                   this, *SenderAddr->ToString(true));
            continue;
        }

        // Null-terminate and convert to FString
        Buffer[FMath::Min(BytesRead, BufferSize - 1)] = 0;
        FString JsonStr = FString(UTF8_TO_TCHAR(reinterpret_cast<const char*>(Buffer.GetData())));

        UE_LOG(LogQuestAnchor, Log, TEXT("[%p] Received %d bytes from %s: %s"),
               this, BytesRead, *SenderAddr->ToString(true), *JsonStr);

        // Parse packet — bIsValid=true for "ready", false for "not_found"
        FAnchorTransformData ParsedData;
        ParsePacket(JsonStr, ParsedData);

        // Update cached anchor only when valid
        if (ParsedData.bIsValid)
        {
            FScopeLock Lock(&DataLock);
            LastAnchorData = ParsedData;
        }

        AsyncTask(ENamedThreads::GameThread, [this, ParsedData]()
        {
            CancelTimeoutTimer();
            OnAnchorReceived.Broadcast(ParsedData);
        });
    }

    return 0;
}

void UAnchorReceiverSubsystem::Stop()
{
    bShouldRun = false;
}

// ---------------------------------------------------------------------------
// Blueprint API
// ---------------------------------------------------------------------------

void UAnchorReceiverSubsystem::RequestAnchor(const FString& QuestIPAddress, float TimeoutSeconds)
{
    if (!ListenSocket)
    {
        UE_LOG(LogQuestAnchor, Warning, TEXT("RequestAnchor: socket not ready"));
        return;
    }

    ISocketSubsystem* SocketSubsystem = ISocketSubsystem::Get(PLATFORM_SOCKETSUBSYSTEM);
    TSharedRef<FInternetAddr> TargetAddr = SocketSubsystem->CreateInternetAddr();

    bool bValidIP = false;
    TargetAddr->SetIp(*QuestIPAddress, bValidIP);
    if (!bValidIP)
    {
        UE_LOG(LogQuestAnchor, Warning, TEXT("RequestAnchor: invalid IP address: %s"), *QuestIPAddress);
        return;
    }
    TargetAddr->SetPort(QuestPort);

    // Mark a request as in-flight so the receive thread cancels the timer on response
    bPendingRequest = true;

    // Send a minimal one-byte query — Quest responds with current anchor JSON
    const uint8 QueryByte = '?';
    int32 BytesSent = 0;
    ListenSocket->SendTo(&QueryByte, 1, BytesSent, *TargetAddr);

    UE_LOG(LogQuestAnchor, Log, TEXT("RequestAnchor: query sent to %s:%d (timeout=%.1fs)"),
           *QuestIPAddress, QuestPort, TimeoutSeconds);

    // Start timeout timer — fires OnAnchorRequestTimeout if Quest doesn't respond
    UWorld* World = GetGameInstance() ? GetGameInstance()->GetWorld() : nullptr;
    if (World)
    {
        World->GetTimerManager().ClearTimer(RequestTimeoutHandle);
        World->GetTimerManager().SetTimer(
            RequestTimeoutHandle,
            [this]()
            {
                bPendingRequest = false;
                UE_LOG(LogQuestAnchor, Warning,
                       TEXT("RequestAnchor: timeout — Quest unreachable or app not running"));
                OnAnchorRequestTimeout.Broadcast();
            },
            TimeoutSeconds,
            false   // not looping
        );
    }
}

void UAnchorReceiverSubsystem::CancelTimeoutTimer()
{
    // Must run on Game Thread (FTimerManager is not thread-safe)
    UWorld* World = GetGameInstance() ? GetGameInstance()->GetWorld() : nullptr;
    if (World)
    {
        World->GetTimerManager().ClearTimer(RequestTimeoutHandle);
    }
}

FAnchorTransformData UAnchorReceiverSubsystem::GetLastAnchorTransform() const
{
    FScopeLock Lock(&DataLock);
    return LastAnchorData;
}

bool UAnchorReceiverSubsystem::IsAnchorValid() const
{
    FScopeLock Lock(&DataLock);
    return LastAnchorData.bIsValid;
}

// ---------------------------------------------------------------------------
// Packet parsing
// ---------------------------------------------------------------------------

bool UAnchorReceiverSubsystem::ParsePacket(const FString& JsonStr, FAnchorTransformData& OutData) const
{
    TSharedPtr<FJsonObject> Root;
    TSharedRef<TJsonReader<>> Reader = TJsonReaderFactory<>::Create(JsonStr);

    if (!FJsonSerializer::Deserialize(Reader, Root) || !Root.IsValid())
    {
        UE_LOG(LogQuestAnchor, Warning, TEXT("JSON parse failed: %s"), *JsonStr);
        return false;
    }

    // Version check
    int32 Version = 0;
    Root->TryGetNumberField(TEXT("version"), Version);
    if (Version != 1)
    {
        UE_LOG(LogQuestAnchor, Warning, TEXT("Unsupported anchor packet version: %d"), Version);
        return false;
    }

    // Status check — "not_found" means Quest hasn't located an anchor yet
    FString Status;
    if (Root->TryGetStringField(TEXT("status"), Status) && Status != TEXT("ready"))
    {
        UE_LOG(LogQuestAnchor, Log, TEXT("Anchor status: %s (no data yet)"), *Status);
        return false;
    }

    // UUID
    FString UUID;
    if (!Root->TryGetStringField(TEXT("uuid"), UUID))
    {
        UE_LOG(LogQuestAnchor, Warning, TEXT("Missing 'uuid' field"));
        return false;
    }

    // Position
    const TSharedPtr<FJsonObject>* PosObj = nullptr;
    if (!Root->TryGetObjectField(TEXT("position"), PosObj) || !PosObj)
    {
        UE_LOG(LogQuestAnchor, Warning, TEXT("Missing 'position' field"));
        return false;
    }
    double px = 0, py = 0, pz = 0;
    (*PosObj)->TryGetNumberField(TEXT("x"), px);
    (*PosObj)->TryGetNumberField(TEXT("y"), py);
    (*PosObj)->TryGetNumberField(TEXT("z"), pz);

    // Orientation
    const TSharedPtr<FJsonObject>* RotObj = nullptr;
    if (!Root->TryGetObjectField(TEXT("orientation"), RotObj) || !RotObj)
    {
        UE_LOG(LogQuestAnchor, Warning, TEXT("Missing 'orientation' field"));
        return false;
    }
    double qx = 0, qy = 0, qz = 0, qw = 1;
    (*RotObj)->TryGetNumberField(TEXT("x"), qx);
    (*RotObj)->TryGetNumberField(TEXT("y"), qy);
    (*RotObj)->TryGetNumberField(TEXT("z"), qz);
    (*RotObj)->TryGetNumberField(TEXT("w"), qw);

    OutData.UUID      = UUID;
    OutData.Transform = ConvertOpenXRToUE(
        (float)px, (float)py, (float)pz,
        (float)qx, (float)qy, (float)qz, (float)qw);
    OutData.bIsValid  = true;

    return true;
}

// ---------------------------------------------------------------------------
// Coordinate conversion
// ---------------------------------------------------------------------------

FTransform UAnchorReceiverSubsystem::ConvertOpenXRToUE(
    float px, float py, float pz,
    float qx, float qy, float qz, float qw)
{
    // OpenXR STAGE: right-handed, Y-up, -Z forward, centimetres (Quest sends cm)
    // Unreal Engine: left-handed, Z-up,  X forward, centimetres
    //
    // Axis mapping (Epic's OpenXR plugin convention):
    //   UE.X = -OXR.Z   (UE forward  = negative OpenXR forward)
    //   UE.Y =  OXR.X   (UE right    = OpenXR right)
    //   UE.Z =  OXR.Y   (UE up       = OpenXR up)
    //
    // Scale: input already in cm — NO ×100 (Quest converts m→cm at the output boundary).
    const FVector Location(-pz, px, py);

    // Quaternion: apply the same axis remap, negate W for handedness flip.
    // Reference: Epic's OpenXRHMD.cpp  ToFQuat(XrQuaternionf)
    //   new_qx = -OXR.qz   (around new X-axis)
    //   new_qy =  OXR.qx   (around new Y-axis)
    //   new_qz =  OXR.qy   (around new Z-axis)
    //   new_qw = -OXR.qw   (handedness sign flip)
    // ⚠ Validate rotation direction against a physical object on first integration.
    FQuat Rotation(-qz, qx, qy, -qw);
    Rotation.Normalize();

    return FTransform(Rotation, Location, FVector::OneVector);
}
