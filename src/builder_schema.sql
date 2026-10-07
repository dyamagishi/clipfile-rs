-- Initial raster-document schema derived from clip-file/builder_constants.py.
-- Reverse-engineered CSP metadata, not an official format specification.
CREATE TABLE Project (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, ProjectInternalVersion TEXT,
 ProjectName TEXT, ProjectCanvas INTEGER, ProjectItemBank INTEGER,
 ProjectCutBank INTEGER, ProjectLayerCompManager INTEGER, ProjectRootCanvasNode INTEGER
);
CREATE TABLE Canvas (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasUnit INTEGER,
 CanvasWidth REAL, CanvasHeight REAL, CanvasResolution REAL, CanvasChannelBytes INTEGER,
 CanvasDefaultChannelOrder INTEGER, CanvasRootFolder INTEGER, CanvasCurrentLayer INTEGER,
 CanvasDoSimulateColor INTEGER, CanvasSrcProfileName, CanvasSrcProfile,
 CanvasDstProfileName, CanvasDstProfile, CanvasRenderingIntent, CanvasUseLibraryType,
 CanvasSimulateSrcProfileName, CanvasSimulateSrcProfile, CanvasSimulateDstProfileName,
 CanvasSimulateDstProfile, CanvasSimulateRenderingIntent, CanvasSimulateUseLibraryType,
 CanvasUseColorAdjustment, CanvasColorAdjustmentToneCurve, CanvasColorAdjustmentLevel,
 CanvasDefaultColorTypeIndex INTEGER, CanvasDefaultColorBlackChecked INTEGER,
 CanvasDefaultColorWhiteChecked INTEGER, CanvasDefaultToneLine REAL, CanvasDoublePage INTEGER,
 CanvasRenderMipmapForceSaved INTEGER, CanvasWorkTime INTEGER, Canvas3DModelDataLoaderIndex INTEGER
);
CREATE TABLE CanvasItem (_PW_ID INTEGER PRIMARY KEY AUTOINCREMENT);
CREATE TABLE CanvasItemBank (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER,
 ModelBankMainIndex INTEGER, BankRootItemMainIndex INTEGER
);
CREATE TABLE AnimationCutBank (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, FirstTimeLine INTEGER,
 FirstScenario INTEGER, CurrentIndex INTEGER, Enable INTEGER, FlagScenarioV155
);
CREATE TABLE Layer (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasId INTEGER,
 LayerName TEXT, LayerType INTEGER, LayerLock INTEGER, LayerClip INTEGER,
 LayerMasking INTEGER, LayerOffsetX INTEGER, LayerOffsetY INTEGER,
 LayerRenderOffscrOffsetX INTEGER, LayerRenderOffscrOffsetY INTEGER,
 LayerMaskOffsetX INTEGER, LayerMaskOffsetY INTEGER,
 LayerMaskOffscrOffsetX INTEGER, LayerMaskOffscrOffsetY INTEGER,
 LayerOpacity INTEGER, LayerComposite INTEGER, LayerUsePaletteColor INTEGER,
 LayerNoticeablePaletteColor INTEGER, LayerPaletteRed INTEGER, LayerPaletteGreen INTEGER,
 LayerPaletteBlue INTEGER, LayerFolder INTEGER, LayerVisibility INTEGER, LayerSelect INTEGER,
 LayerNextIndex INTEGER, LayerFirstChildIndex INTEGER, LayerUuid TEXT,
 LayerRenderMipmap INTEGER, LayerLayerMaskMipmap INTEGER, LayerRenderThumbnail INTEGER,
 LayerLayerMaskThumbnail INTEGER, UsePreviewColorType, UsePreviewMaskColorType,
 EffectRangeType, DraftLayer, FilterLayerV132, FilterLayerColorMixingInfo,
 GradationFillColorMixingInfo, LightTableInfo BLOB
);
CREATE TABLE LayerComp (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, BankId INTEGER,
 CompNextIndex INTEGER, CompName TEXT, CompUuid BLOB, CompLayerInfo BLOB
);
CREATE TABLE LayerCompManager (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, FirstLayerCompIndex INTEGER,
 LastStateLayerCompIndex INTEGER, AppliedLayerCompIndex INTEGER
);
CREATE TABLE LayerThumbnail (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
 ThumbnailSmallerNeedRefresh INTEGER, ThumbnailSmallNeedRefresh INTEGER,
 ThumbnailMiddleNeedRefresh INTEGER, ThumbnailLargeNeedRefresh INTEGER,
 ThumbnailLargerNeedRefresh INTEGER, ThumbnailMiddle2xNeedRefresh INTEGER,
 ThumbnailLarger2xNeedRefresh INTEGER, ThumbnailSmallerNeedRefresh1 INTEGER,
 ThumbnailSmallNeedRefresh1 INTEGER, ThumbnailMiddleNeedRefresh1 INTEGER,
 ThumbnailLargeNeedRefresh1 INTEGER, ThumbnailLargerNeedRefresh1 INTEGER,
 ThumbnailMiddle2xNeedRefresh1 INTEGER, ThumbnailLarger2xNeedRefresh1 INTEGER,
 ThumbnailDrewMode INTEGER, ThumbnailFixMode INTEGER, ThumbnailCanvasWidth INTEGER,
 ThumbnailCanvasHeight INTEGER, ThumbnailUseDrawColor INTEGER,
 ThumbnailMainColorRed INTEGER, ThumbnailMainColorGreen INTEGER, ThumbnailMainColorBlue INTEGER,
 ThumbnailSubColorRed INTEGER, ThumbnailSubColorGreen INTEGER, ThumbnailSubColorBlue INTEGER,
 ThumbnailOffscreen INTEGER, ThumbnailColorTypeIndex INTEGER, ThumbnailColorTypeBlack INTEGER,
 ThumbnailColorTypeWhite INTEGER, ThumbnailPrewviewColorTypeIndex INTEGER,
 ThumbnailPrewviewColorTypeBlack INTEGER, ThumbnailPrewviewColorTypeWhite INTEGER,
 ThumbnailPrewviewColorTypeOpacity INTEGER, ThumbnailPrewviewColorTypeImage INTEGER,
 ThumbnailPrewviewColorTypeAlpha INTEGER, ThumbnailPrewviewMaskBinarize INTEGER,
 ThumbnailPrewviewMaskThreshold INTEGER, ThumbnailDrewUseCanvasAspect0 INTEGER,
 ThumbnailDrewUseCanvasAspect1 INTEGER
);
CREATE TABLE Mipmap (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
 MipmapCount INTEGER, BaseMipmapInfo INTEGER
);
CREATE TABLE MipmapInfo (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
 ThisScale REAL, Offscreen INTEGER, NextIndex INTEGER
);
CREATE TABLE Offscreen (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasId INTEGER, LayerId INTEGER,
 Attribute BLOB, BlockData BLOB
);
CREATE TABLE CanvasPreview (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, MainId INTEGER, CanvasId INTEGER,
 ImageType INTEGER, ImageWidth INTEGER, ImageHeight INTEGER, ImageData BLOB
);
CREATE TABLE ElemScheme (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, TableName TEXT, ElemType INTEGER, MaxIndex INTEGER
);
CREATE TABLE ParamScheme (
 _PW_ID INTEGER PRIMARY KEY AUTOINCREMENT, TableName TEXT, LabelName TEXT,
 DataType INTEGER, Flag INTEGER, OwnerType INTEGER, LockType INTEGER, LockSpecified INTEGER,
 AlternativeLockType, AlternativeLockSpecified, LinkTable TEXT
);
CREATE TABLE ExternalChunk (ExternalID BLOB, Offset INTEGER);
CREATE TABLE ExternalTableAndColumnName (TableName TEXT, ColumnName TEXT);
CREATE TABLE RemovedExternal (ExternalID BLOB);
