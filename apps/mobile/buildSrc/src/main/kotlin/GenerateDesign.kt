import com.squareup.kotlinpoet.ClassName
import com.squareup.kotlinpoet.CodeBlock
import com.squareup.kotlinpoet.FLOAT
import com.squareup.kotlinpoet.FileSpec
import com.squareup.kotlinpoet.FunSpec
import com.squareup.kotlinpoet.INT
import com.squareup.kotlinpoet.KModifier
import com.squareup.kotlinpoet.LIST
import com.squareup.kotlinpoet.MAP
import com.squareup.kotlinpoet.MemberName
import com.squareup.kotlinpoet.ParameterizedTypeName.Companion.parameterizedBy
import com.squareup.kotlinpoet.PropertySpec
import com.squareup.kotlinpoet.STRING
import com.squareup.kotlinpoet.TypeName
import com.squareup.kotlinpoet.TypeSpec
import com.squareup.kotlinpoet.joinToCode
import groovy.json.JsonSlurper
import java.io.File
import kotlin.math.roundToInt
import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction

private const val UNIT = "androidx.compose.ui.unit"
private val COLOR = ClassName("androidx.compose.ui.graphics", "Color")
private val DP = ClassName(UNIT, "Dp")
private val TEXT_UNIT = ClassName(UNIT, "TextUnit")
private val EASING = ClassName("androidx.compose.animation.core", "CubicBezierEasing")
private val DP_OF = MemberName(UNIT, "dp", isExtension = true)
private val SP_OF = MemberName(UNIT, "sp", isExtension = true)
private val SHADOW = Regex("""^0 (\d+)px (\d+)px rgba\((\d+), (\d+), (\d+), ([\d.]+)\)$""")

private typealias Json = Map<String, Any>

@Suppress("UNCHECKED_CAST") private fun Any.json() = this as Json

private fun Json.obj(key: String) = getValue(key).json()

private fun Json.number(key: String) = getValue(key) as Number

private fun camel(name: String) = Regex("-(\\w)").replace(name) { it.groupValues[1].uppercase() }

private fun floats(value: Number) = "${value}f"

private fun constant(name: String, type: TypeName, format: String, value: Any) =
    PropertySpec.builder(name, type, KModifier.CONST).initializer(format, value).build()

private fun length(name: String, value: Number) =
    PropertySpec.builder(name, DP).initializer("%L.%M", value, DP_OF).build()

private fun textSize(name: String, value: Number) =
    PropertySpec.builder(name, TEXT_UNIT).initializer("%L.%M", value, SP_OF).build()

private fun dataClass(name: String, fields: List<Pair<String, TypeName>>): TypeSpec {
    val constructor = FunSpec.constructorBuilder()
    val type = TypeSpec.classBuilder(name).addModifiers(KModifier.DATA)
    for ((field, fieldType) in fields) {
        constructor.addParameter(field, fieldType)
        type.addProperty(PropertySpec.builder(field, fieldType).initializer(field).build())
    }
    return type.primaryConstructor(constructor.build()).build()
}

/** `ColorTokens(bg = ..., surface = ...)` 这样的具名构造. */
private fun construct(type: ClassName, arguments: List<Pair<String, CodeBlock>>): CodeBlock {
    val block = CodeBlock.builder().add("%T(\n⇥", type)
    for ((name, value) in arguments) block.add("%L·=·%L,\n", name, value)
    return block.add("⇤)").build()
}

private fun color(argb: String) = CodeBlock.of("%T(0x%L)", COLOR, argb)

private fun Number.hexByte() = toInt().toString(16).padStart(2, '0')

/** CSS 的 box-shadow 拆成层,Kotlin 逐层画. */
private fun shadowLayers(value: String, layer: ClassName): CodeBlock {
    val layers =
        if (value == "none") emptyList()
        else
            value.split(Regex(""",\s*(?=0 \d+px)""")).map { text ->
                val (y, blur, red, green, blue, alpha) = SHADOW.matchEntire(text)!!.destructured
                val argb = listOf((alpha.toDouble() * 255).roundToInt(), red.toInt(), green.toInt(), blue.toInt())
                val hex = argb.joinToString("") { it.hexByte() }.uppercase()
                CodeBlock.of("%T(%L.%M,·%L.%M,·%L)", layer, y, DP_OF, blur, DP_OF, color(hex))
            }
    return CodeBlock.of("listOf(%L)", layers.joinToCode(",·"))
}

private fun lengths(name: String, group: Json) =
    TypeSpec.objectBuilder(name)
        .apply { group.forEach { (k, v) -> addProperty(length(camel(k), v as Number)) } }
        .build()

private fun textSizes(name: String, group: Json) =
    TypeSpec.objectBuilder(name)
        .apply { group.forEach { (k, v) -> addProperty(textSize(camel(k), v as Number)) } }
        .build()

private fun motion(tokens: Json): TypeSpec {
    val motion = tokens.obj("motion")
    val type = TypeSpec.objectBuilder("Motion").addKdoc("动效令牌.时长单位是毫秒.")
    motion.obj("duration").forEach { (name, value) -> type.addProperty(constant(name, INT, "%L", value)) }
    for ((name, curve) in motion.obj("easing")) {
        val points = (curve as List<*>).joinToString(",·") { floats(it as Number) }
        type.addProperty(PropertySpec.builder("${name}Easing", EASING).initializer("%T(%L)", EASING, points).build())
    }
    for (name in listOf("press-scale", "pop-scale", "scrim")) {
        type.addProperty(constant(camel(name), FLOAT, "%L", floats(motion.number(name))))
    }
    type.addProperty(constant("staggerMax", INT, "%L", motion.number("stagger-max")))
    for (name in listOf("page-in", "page-out", "rise")) type.addProperty(length(camel(name), motion.number(name)))
    return type.build()
}

private fun qr(tokens: Json): TypeSpec {
    val qr = tokens.obj("qr")
    val intervals = qr.obj("interval").values.joinToString(",·")
    return TypeSpec.objectBuilder("Qr")
        .addKdoc("二维码的静区,分段上限和轮播间隔,两端相同.间隔单位是毫秒,第一项是默认速度.")
        .addProperty(constant("quiet", INT, "%L", qr.number("quiet")))
        .addProperty(constant("maxSegments", INT, "%L", qr.number("max-segments")))
        .addProperty(
            PropertySpec.builder("intervals", LIST.parameterizedBy(INT)).initializer("listOf(%L)", intervals).build()
        )
        .build()
}

private fun theme(tokens: Json, pkg: String): FileSpec {
    val colors = tokens.obj("color")
    val elevation = tokens.obj("elevation")
    val colorTokens = ClassName(pkg, "ColorTokens")
    val elevationTokens = ClassName(pkg, "ElevationTokens")
    val layer = ClassName(pkg, "ShadowLayer")
    fun palette(name: String, mode: String): PropertySpec {
        val entries = colors.map { (key, value) ->
            val hex = (value.json().getValue(mode) as String).removePrefix("#")
            camel(key) to color("FF$hex")
        }
        return PropertySpec.builder(name, colorTokens).initializer(construct(colorTokens, entries)).build()
    }
    val shadows = elevation.map { (key, value) ->
        camel(key) to shadowLayers(value.json().getValue("dark") as String, layer)
    }
    return FileSpec.builder(pkg, "Tokens")
        .addType(dataClass("ColorTokens", colors.keys.map { camel(it) to COLOR }))
        .addProperty(palette("LightColors", "light"))
        .addProperty(palette("DarkColors", "dark"))
        .addType(
            dataClass("ShadowLayer", listOf("y" to DP, "blur" to DP, "color" to COLOR))
                .toBuilder()
                .addKdoc("一层阴影,对应 CSS box-shadow 的纵向偏移,模糊半径和颜色.")
                .build()
        )
        .addType(dataClass("ElevationTokens", elevation.keys.map { camel(it) to LIST.parameterizedBy(layer) }))
        .addProperty(
            PropertySpec.builder("DarkElevation", elevationTokens)
                .initializer(construct(elevationTokens, shadows))
                .build()
        )
        .addType(lengths("Space", tokens.obj("space")))
        .addType(lengths("Radius", tokens.obj("radius")))
        .addType(textSizes("FontSize", tokens.obj("fontSize")))
        .addType(textSizes("LineHeight", tokens.obj("lineHeight")))
        .addType(lengths("Size", tokens.obj("size")))
        .addType(motion(tokens))
        .addType(qr(tokens))
        .build()
}

/** 线性图标的路径.网页端按名字查,冷钱包是常量,名字首字母大写. */
private fun icons(paths: Json, pkg: String): FileSpec {
    val icons = TypeSpec.objectBuilder("Icons")
    for ((name, path) in paths) {
        icons.addProperty(constant(name.replaceFirstChar(Char::uppercase), STRING, "%S", path))
    }
    return FileSpec.builder(pkg, "IconPaths").addType(icons.build()).build()
}

/** 词典以中文原文为键,网页端和冷钱包共用. */
private fun translations(english: Json, pkg: String): FileSpec {
    val entries = CodeBlock.builder().add("mapOf(\n⇥")
    for ((chinese, text) in english) entries.add("%S·to·%S,\n", chinese, text)
    val property =
        PropertySpec.builder("EN", MAP.parameterizedBy(STRING, STRING), KModifier.INTERNAL)
            .initializer(entries.add("⇤)").build())
            .build()
    return FileSpec.builder(pkg, "Translations").addProperty(property).build()
}

/** 从 design/ 的 JSON 生成令牌,图标和词典的 Kotlin 源码,输出到构建目录. */
abstract class GenerateDesign : DefaultTask() {
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val tokens: RegularFileProperty
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val icons: RegularFileProperty
    @get:InputFile @get:PathSensitive(PathSensitivity.NONE) abstract val dictionary: RegularFileProperty
    @get:Input abstract val rootPackage: Property<String>
    @get:OutputDirectory abstract val output: DirectoryProperty

    private fun read(file: RegularFileProperty) = JsonSlurper().parseText(file.get().asFile.readText()).json()

    @TaskAction
    fun generate() {
        val root = rootPackage.get()
        val directory: File = output.get().asFile
        directory.deleteRecursively()
        theme(read(tokens), "$root.ui.theme").writeTo(directory)
        icons(read(icons), "$root.ui.components").writeTo(directory)
        translations(read(dictionary), "$root.i18n").writeTo(directory)
    }
}
