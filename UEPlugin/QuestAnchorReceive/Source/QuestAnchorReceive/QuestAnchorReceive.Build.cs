// Copyright Epic Games, Inc. All Rights Reserved.

using UnrealBuildTool;

public class QuestAnchorReceive : ModuleRules
{
	public QuestAnchorReceive(ReadOnlyTargetRules Target) : base(Target)
	{
		PCHUsage = ModuleRules.PCHUsageMode.UseExplicitOrSharedPCHs;
		
		PublicIncludePaths.AddRange(
			new string[] {
				// ... add public include paths required here ...
			}
			);
				
		
		PrivateIncludePaths.AddRange(
			new string[] {
				// ... add other private include paths required here ...
			}
			);
			
		
		PublicDependencyModuleNames.AddRange(
			new string[]
			{
				"Core",
				"CoreUObject",
				"Engine",
			}
			);


		PrivateDependencyModuleNames.AddRange(
			new string[]
			{
				"Sockets",        // FSocket, ISocketSubsystem
				"Networking",     // FIPv4Endpoint, FUdpSocketReceiver
				"Json",           // FJsonObject, FJsonSerializer
				"JsonUtilities",  // FJsonObjectConverter
			}
			);
		
		
		DynamicallyLoadedModuleNames.AddRange(
			new string[]
			{
				// ... add any modules that your module loads dynamically here ...
			}
			);
	}
}
