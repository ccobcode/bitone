package com.ccobcode.bitone

import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import com.ccobcode.bitone.ui.theme.BitOneFonts

/** 字体打包进应用,不依赖各厂商的系统字体.文件由 tools/fonts/build_fonts.py 生成. */
val BundledFonts =
    BitOneFonts(
        sans =
            FontFamily(
                Font(R.font.bitone_sans_regular, FontWeight.Normal),
                Font(R.font.bitone_sans_medium, FontWeight.Medium),
                Font(R.font.bitone_sans_semibold, FontWeight.SemiBold),
            ),
        mono =
            FontFamily(
                Font(R.font.bitone_mono_regular, FontWeight.Normal),
                Font(R.font.bitone_mono_medium, FontWeight.Medium),
                Font(R.font.bitone_mono_semibold, FontWeight.SemiBold),
            ),
    )
