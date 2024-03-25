Invoke-WebRequest https://download.steinberg.net/sdk_downloads/asiosdk_2.3.3_2019-06-14.zip -OutFile asiosdk_2.3.3_2019-06-14.zip

Expand-Archive -Path asiosdk_2.3.3_2019-06-14.zip -DestinationPath .

Rename-Item -Path asiosdk_2.3.3_2019-06-14 -NewName asiosdk

Remove-Item -Path asiosdk_2.3.3_2019-06-14.zip


Invoke-WebRequest https://npcap.com/dist/npcap-sdk-1.13.zip -OutFile npcap-sdk-1.13.zip

Expand-Archive -Path npcap-sdk-1.13.zip -DestinationPath npcap

Remove-Item -Path npcap-sdk-1.13.zip


Invoke-WebRequest https://www.wintun.net/builds/wintun-0.14.1.zip -OutFile wintun-0.14.1.zip

Expand-Archive -Path wintun-0.14.1.zip -DestinationPath .

Remove-Item -Path wintun-0.14.1.zip
