#pragma once

#include "CoreMinimal.h"
#include "AnchorTransformData.generated.h"

/**
 * Received spatial anchor data from the ALVR Quest client.
 * Transform is already converted to Unreal Engine coordinate space (cm, Z-up, left-handed).
 */
USTRUCT(BlueprintType)
struct FAnchorTransformData
{
    GENERATED_BODY()

    /** OpenXR UUID string of the spatial anchor */
    UPROPERTY(BlueprintReadOnly, Category = "QuestAnchor")
    FString UUID;

    /**
     * Anchor world transform in UE space (centimetres, Z-up, X-forward, left-handed).
     * Converted from OpenXR STAGE space (Y-up, right-handed; Quest already sends cm)
     * using Epic's convention:
     *   UE.Location = FVector(-OXR.z, OXR.x, OXR.y)   // no ×100: input is already cm
     *   UE.Rotation = FQuat(-OXR.qz, OXR.qx, OXR.qy, -OXR.qw)
     * NOTE: validate rotation sign with a physical reference after first integration.
     */
    UPROPERTY(BlueprintReadOnly, Category = "QuestAnchor")
    FTransform Transform = FTransform::Identity;

    /** True once at least one valid packet has been received */
    UPROPERTY(BlueprintReadOnly, Category = "QuestAnchor")
    bool bIsValid = false;
};
