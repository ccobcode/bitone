# 只裁剪和优化，不混淆，崩溃堆栈和发布包审计都看得懂。
-dontobfuscate

# JNA 和 UniFFI 靠反射找方法和字段，原生库按名字回调。
-keep class com.sun.jna.** { *; }
-keep class * extends com.sun.jna.** { *; }
-keep class uniffi.bitone_ffi.** { *; }
-dontwarn java.awt.**

# zxing-cpp 的原生库按名字构造结果对象。
-keep class zxingcpp.** { *; }
