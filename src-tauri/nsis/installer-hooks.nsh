!macro NSIS_HOOK_POSTINSTALL
  CopyFiles /SILENT "$INSTDIR\_up_\.tools\cuda-runtime\*.dll" "$INSTDIR"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Delete "$INSTDIR\cublas64_*.dll"
  Delete "$INSTDIR\cublasLt64_*.dll"
  Delete "$INSTDIR\cudart64_*.dll"
  Delete "$INSTDIR\curand64_*.dll"
  Delete "$INSTDIR\nvrtc64_*.dll"
  Delete "$INSTDIR\nvrtc-builtins64_*.dll"
  Delete "$INSTDIR\nvJitLink_*.dll"
  Delete "$INSTDIR\vcomp140.dll"
!macroend
