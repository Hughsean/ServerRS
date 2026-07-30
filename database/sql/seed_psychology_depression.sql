-- Digital human: production-safe seed data for depression scales and psychology knowledge.
-- Generated 2026-07-30. This script is idempotent for the deterministic IDs below.
-- It intentionally does not create depression_assessments: assessments must come from real users.

SET NAMES utf8mb4 COLLATE utf8mb4_unicode_ci;
START TRANSACTION;

INSERT INTO depression_scales
    (scale_id, scale_name, scale_description, min_score, max_score, severity_ranges, questions)
VALUES
(1, 'PHQ-9',
 '患者健康问卷抑郁量表（PHQ-9）完整版。评估过去两周9类抑郁症状，每题0–3分，总分0–27分。常用分界点为5、10、15、20分。该量表仅用于筛查和症状监测，不能代替诊断；无论总分多少，第9题只要非0分都应进一步进行自伤/自杀风险评估。若有立即伤害自己的危险，请联系120/110或心理援助热线12356。',
 0, 27,
 JSON_ARRAY(
   JSON_OBJECT('min',0,'max',4,'level','无或极轻微','label','无或极轻微','guidance','关注日常作息；如持续困扰仍可咨询专业人员'),
   JSON_OBJECT('min',5,'max',9,'level','轻度','label','轻度','guidance','建议观察症状和功能影响，必要时寻求专业评估'),
   JSON_OBJECT('min',10,'max',14,'level','中度','label','中度','guidance','建议预约精神科、心理科或合格心理专业人员评估'),
   JSON_OBJECT('min',15,'max',19,'level','中重度','label','中重度','guidance','建议尽快接受专业评估与干预'),
   JSON_OBJECT('min',20,'max',27,'level','重度','label','重度','guidance','建议尽快接受专业评估；如有自伤危险立即求助')
 ),
 JSON_ARRAY(
   JSON_OBJECT('id',1,'text','做事时提不起兴趣或没有乐趣','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',2,'text','感到心情低落、沮丧或绝望','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',3,'text','入睡困难、睡不安稳或睡眠过多','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',4,'text','感到疲倦或缺乏精力','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',5,'text','食欲不振或吃得过多','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',6,'text','觉得自己很糟、很失败，或让自己或家人失望','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',7,'text','难以集中注意力，例如阅读或看电视时','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',8,'text','动作或说话慢到别人可能察觉，或相反地烦躁、坐立不安','period','过去两周','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天'))),
   JSON_OBJECT('id',9,'text','想到不如死去，或有伤害自己的念头','period','过去两周','risk_item',true,'risk_message','本题非0分时，无论总分高低都应进行进一步风险评估；如存在立即危险，请联系120/110或12356。','options',JSON_ARRAY(JSON_OBJECT('score',0,'label','完全没有'),JSON_OBJECT('score',1,'label','有几天'),JSON_OBJECT('score',2,'label','一半以上时间'),JSON_OBJECT('score',3,'label','几乎每天')))
 )),
(2, 'SDS',
 'Zung抑郁自评量表（SDS）20题完整版，按过去一周症状频率作答。每题1–4分，其中10个正向条目反向计分，原始总分20–80分；中国常用标准分为原始分×1.25后取整数，常用参考界值为53、63、73分。数据库按对应原始分区间分级。仅作筛查，不代替临床诊断。第19题涉及死亡想法，非最低选项时应进一步评估风险。',
 20, 80,
 JSON_ARRAY(
   JSON_OBJECT('min',20,'max',41,'level','正常范围','label','正常范围','standard_score','25–52'),
   JSON_OBJECT('min',42,'max',49,'level','轻度','label','轻度','standard_score','53–62'),
   JSON_OBJECT('min',50,'max',57,'level','中度','label','中度','standard_score','63–72'),
   JSON_OBJECT('min',58,'max',80,'level','重度','label','重度','standard_score','73–100')
 ),
 JSON_ARRAY(
   JSON_OBJECT('id',1,'text','我感到情绪低沉、郁闷','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',2,'text','我在早晨心情最好','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',3,'text','我会突然哭出来或想哭','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',4,'text','我夜间睡眠不好','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',5,'text','我的食量跟平常一样','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',6,'text','我仍然享有正常的性兴趣','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',7,'text','我感到体重在下降','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',8,'text','我有便秘的烦恼','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',9,'text','我的心跳比平时快','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',10,'text','我无缘无故感到疲乏','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',11,'text','我的头脑跟平常一样清楚','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',12,'text','我做事情跟平常一样不觉得困难','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',13,'text','我坐卧不安，难以保持平静','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',14,'text','我对未来抱有希望','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',15,'text','我比平常更容易激动或烦躁','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',16,'text','我觉得作决定是容易的','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',17,'text','我觉得自己是有用和被需要的人','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',18,'text','我的生活过得很有意义','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',19,'text','我认为如果我死了，别人会生活得更好','reverse_scored',false,'risk_item',true,'risk_message','本题不是最低频率时，应进一步进行自伤/自杀风险评估；如存在立即危险，请联系120/110或12356。','options',JSON_ARRAY(JSON_OBJECT('score',1,'label','没有或很少'),JSON_OBJECT('score',2,'label','有时'),JSON_OBJECT('score',3,'label','大部分时间'),JSON_OBJECT('score',4,'label','绝大部分或全部时间'))),
   JSON_OBJECT('id',20,'text','平常感兴趣的事我仍然感兴趣','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',4,'label','没有或很少'),JSON_OBJECT('score',3,'label','有时'),JSON_OBJECT('score',2,'label','大部分时间'),JSON_OBJECT('score',1,'label','绝大部分或全部时间')))
 )),
(3, 'CES-D',
 '流调中心抑郁量表（CES-D）20题版，用于了解过去一周抑郁症状出现频率。每题0–3分，第4、8、12、16题反向计分，总分0–60分；16分是原量表常用筛查界值，但不同人群的最佳界值可能不同。结果只表示需要进一步评估的可能性，不等于抑郁症诊断。',
 0, 60,
 JSON_ARRAY(
   JSON_OBJECT('min',0,'max',15,'level','未达到常用筛查界值','label','未达到常用筛查界值','guidance','如症状持续或影响生活，仍建议咨询专业人员'),
   JSON_OBJECT('min',16,'max',60,'level','达到常用筛查界值','label','达到常用筛查界值','guidance','建议由精神科、心理科或合格心理专业人员进一步评估')
 ),
 JSON_ARRAY(
   JSON_OBJECT('id',1,'text','一些平时不困扰我的事也让我烦恼','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',2,'text','我不想吃东西，胃口不好','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',3,'text','即使家人朋友帮助我，我仍难以摆脱苦闷','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',4,'text','我觉得自己和别人一样好','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',3,'label','少于1天'),JSON_OBJECT('score',2,'label','1–2天'),JSON_OBJECT('score',1,'label','3–4天'),JSON_OBJECT('score',0,'label','5–7天'))),
   JSON_OBJECT('id',5,'text','我难以集中注意力做事','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',6,'text','我感到情绪低落','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',7,'text','我觉得做任何事都很费力','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',8,'text','我对未来抱有希望','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',3,'label','少于1天'),JSON_OBJECT('score',2,'label','1–2天'),JSON_OBJECT('score',1,'label','3–4天'),JSON_OBJECT('score',0,'label','5–7天'))),
   JSON_OBJECT('id',9,'text','我觉得过去的生活是失败的','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',10,'text','我感到害怕','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',11,'text','我的睡眠不好','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',12,'text','我感到高兴','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',3,'label','少于1天'),JSON_OBJECT('score',2,'label','1–2天'),JSON_OBJECT('score',1,'label','3–4天'),JSON_OBJECT('score',0,'label','5–7天'))),
   JSON_OBJECT('id',13,'text','我比平时说话少','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',14,'text','我感到孤单','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',15,'text','人们对我不友好','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',16,'text','我享受生活','reverse_scored',true,'options',JSON_ARRAY(JSON_OBJECT('score',3,'label','少于1天'),JSON_OBJECT('score',2,'label','1–2天'),JSON_OBJECT('score',1,'label','3–4天'),JSON_OBJECT('score',0,'label','5–7天'))),
   JSON_OBJECT('id',17,'text','我有过哭泣','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',18,'text','我感到悲伤','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',19,'text','我觉得别人不喜欢我','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天'))),
   JSON_OBJECT('id',20,'text','我提不起劲来做事','reverse_scored',false,'options',JSON_ARRAY(JSON_OBJECT('score',0,'label','少于1天'),JSON_OBJECT('score',1,'label','1–2天'),JSON_OBJECT('score',2,'label','3–4天'),JSON_OBJECT('score',3,'label','5–7天')))
 ))
ON DUPLICATE KEY UPDATE
 scale_name=VALUES(scale_name), scale_description=VALUES(scale_description), min_score=VALUES(min_score),
 max_score=VALUES(max_score), severity_ranges=VALUES(severity_ranges), questions=VALUES(questions);

INSERT INTO psychology_categories
    (category_id, category_name, parent_id, description, sort_order, status)
VALUES
 (1,'抑郁科普',NULL,'抑郁症识别、筛查、治疗与康复知识',10,1),
 (2,'情绪管理',NULL,'理解和调节常见情绪反应的方法',20,1),
 (3,'焦虑应对',NULL,'担忧、惊恐与焦虑相关的科学应对',30,1),
 (4,'睡眠改善',NULL,'睡眠节律、失眠与睡眠卫生知识',40,1),
 (5,'正念放松',NULL,'正念、呼吸和身体放松练习',50,1),
 (6,'人际关系',NULL,'沟通、边界、支持与关系修复',60,1),
 (7,'自我成长',NULL,'自我关怀、韧性、价值与行动',70,1),
 (8,'压力管理',NULL,'工作学习压力、倦怠与恢复',80,1),
 (9,'重点人群',NULL,'青少年、孕产期和老年人等人群心理健康',90,1),
 (10,'危机与求助',NULL,'风险识别、安全计划和专业求助渠道',100,1)
ON DUPLICATE KEY UPDATE category_name=VALUES(category_name), parent_id=VALUES(parent_id),
 description=VALUES(description), sort_order=VALUES(sort_order), status=VALUES(status);

INSERT INTO psychology_articles
    (article_id, category_id, title, summary, content, author, source, tags, cover_image,
     view_count, like_count, is_featured, is_published, publish_date)
VALUES
(1,1,'抑郁症不只是心情不好：如何识别持续性信号',
 '区分短暂情绪低落与需要专业关注的抑郁症状。',
 '每个人都会有低落的时候，但抑郁症通常表现为低落、空虚或兴趣明显下降，并在大部分时间、几乎每天持续至少两周。它还可能伴随睡眠或食欲变化、疲惫、注意力下降、自责、绝望以及死亡相关想法，并影响学习、工作和关系。\n\n抑郁不是意志薄弱，也不能只靠“想开点”解决。生物、心理和社会因素会共同影响一个人的状态。若症状持续、反复或功能明显受损，应到精神科、心理科或正规医疗机构评估。筛查量表能帮助发现风险，但不能单独作出诊断。\n\n如出现伤害自己的念头或计划，请不要独处，远离可用于伤害自己的物品，立即联系可信任的人、120/110或心理援助热线12356。',
 '数字人心理健康编辑组','世界卫生组织（WHO）抑郁症专题','["抑郁症","症状识别","专业求助"]',NULL,0,0,1,1,'2026-07-11 09:00:00'),
(2,1,'PHQ-9怎么看：分数之外还要看功能与安全',
 '说明PHQ-9的时间范围、常用分界点和第9题安全含义。',
 'PHQ-9询问过去两周9类症状出现的频率，每题0至3分，总分0至27分。常用分界点为5、10、15和20分，分数越高通常表示症状负担越重。\n\n解读时不能只看总分：还要看症状是否影响工作、学习、照顾自己和人际关系，也要结合病史、身体疾病、药物、睡眠及是否存在躁狂或轻躁狂经历。尤其第9题涉及死亡或自伤念头，只要不是“完全没有”，无论总分多少都需要进一步安全评估。\n\n量表适合筛查和随访，不等于诊断。结果达到中度及以上、症状明显影响生活，或本人感到难以支撑时，都建议尽快寻求专业帮助。',
 '数字人心理健康编辑组','Kroenke等PHQ-9验证研究；美国NLM CDE','["PHQ-9","量表","筛查"]',NULL,0,0,1,1,'2026-07-12 09:00:00'),
(3,1,'行为激活：情绪没有动力时，从一个小行动开始',
 '用可完成的小行动打破回避、低活动与情绪低落的循环。',
 '抑郁时，人常会减少活动和社交；短期回避似乎省力，长期却可能让成就感、愉悦感和连接感进一步减少。行为激活的核心，是不等“有动力了再做”，而是先安排一个足够小、与生活价值有关的行动。\n\n可以从“十分钟版本”开始：洗澡、到楼下走一圈、回复一条重要消息、整理桌面一角，或按时吃一顿饭。完成后记录行动前后的情绪和精力，不要求立刻开心，只观察是否有一点变化。每周逐步增加有掌控感、愉悦感或连接感的活动。\n\n若你已经难以维持基本生活，或有自伤风险，不要只依赖自助练习，应尽快寻求专业评估。',
 '数字人心理健康编辑组','WHO抑郁症与心理干预资料','["行为激活","抑郁","小行动"]',NULL,0,0,1,1,'2026-07-13 09:00:00'),
(4,2,'识别自动化想法：把“事实”和“解释”分开',
 '一个简洁的认知记录方法，帮助减少灾难化和过度自责。',
 '强烈情绪出现时，大脑会迅速给事件下结论，例如“我一次没做好，所以我什么都不行”。可以用四栏记录：发生了什么、我脑中闪过什么、支持与不支持这个想法的证据、一个更平衡的说法。\n\n更平衡不等于强迫乐观。它可以是：“这次结果不理想，说明这部分需要调整，但不能证明我在所有事情上都失败。”给原想法和替代想法分别打0至100分可信度，再观察情绪变化。\n\n如果反复出现强烈自责、绝望或创伤相关记忆，专业的认知行为治疗会比独自练习更安全、更有针对性。',
 '数字人心理健康编辑组','WHO Doing What Matters in Times of Stress','["认知重构","自动化想法","情绪管理"]',NULL,0,0,0,1,'2026-07-14 09:00:00'),
(5,10,'什么时候该寻求心理或精神科帮助',
 '从持续时间、功能受损和安全风险三个维度判断。',
 '以下情况提示值得预约专业评估：症状持续两周以上；学习、工作、睡眠、饮食或关系明显受影响；依靠酒精或其他物质应对；症状反复出现；自己尝试调整仍无改善；家人朋友明显担忧。\n\n可以先到综合医院精神科、心理科、心身医学科或精神专科医院。就诊前记录症状开始时间、睡眠食欲变化、正在使用的药物和保健品、既往发作、家族史，以及是否出现过异常兴奋、睡得很少却精力旺盛等情况。\n\n若出现具体自伤计划、正在准备工具、无法保证自身安全或已经实施伤害，应按紧急情况处理：不要独处，立即联系120/110、12356和可信任的人。',
 '数字人心理健康编辑组','WHO抑郁症专题；国家卫健委12356政策','["就医","求助","危机"]',NULL,0,0,1,1,'2026-07-15 09:00:00'),
(6,10,'给自己做一份简明安全计划',
 '在危机前写下预警信号、应对步骤和求助联系人。',
 '安全计划最好在相对平稳时完成，并与可信任的人或专业人员讨论。可以依次写下：我进入危机前的预警信号；能让我暂时撑过十分钟的内部应对方法；可以分散注意力的人和地点；能直接谈论危机的联系人；专业机构和紧急联系方式；如何让环境更安全。\n\n把可能用于伤害自己的药物、刀具或其他物品交由可信任的人暂时保管，避免饮酒和独处。计划应放在手机和纸面上，确保容易找到。\n\n安全计划不是治疗的替代品。如果危险正在发生、已有计划或难以控制冲动，请立即联系120/110、心理援助热线12356，或前往最近的急诊。',
 '数字人心理健康编辑组','国家卫健委心理援助热线资料；循证安全计划原则','["安全计划","自伤预防","12356"]',NULL,0,0,1,1,'2026-07-16 09:00:00'),
(7,4,'睡眠和情绪为何互相影响',
 '从规律起床、光照和床的使用开始改善睡眠节律。',
 '睡眠困难可能加重情绪低落和焦虑，而低落与焦虑也会让入睡、早醒或睡眠过多更明显。改善时可优先固定每天起床时间，早晨接触自然光，白天保持适量活动，午后减少咖啡因，睡前降低光线与刺激。\n\n如果躺下后长时间清醒，可以暂时离床，到昏暗处做安静活动，困倦后再回床，让大脑重新建立“床与睡眠”的联系。不要因为一晚没睡好就大幅补觉或过早上床。\n\n持续失眠超过数周、严重打鼾伴呼吸暂停，或睡眠问题与明显抑郁、躁狂症状同时出现时，应接受医疗评估。',
 '数字人心理健康编辑组','循证失眠认知行为治疗原则','["睡眠","失眠","情绪"]',NULL,0,0,0,1,'2026-07-17 09:00:00'),
(8,7,'运动不是“治愈口号”：怎样从低能量状态开始',
 '把活动拆小、降低门槛，并关注可持续性而不是强度。',
 '规律身体活动有助于整体身心健康，也可作为抑郁干预的一部分，但它不是对所有人都足够的单一治疗。低能量时可从5至10分钟步行、伸展或家务开始，把衣服和鞋提前准备好，并选择固定提示，例如午饭后出门。\n\n目标应小到“状态不好也能做”。完成比强度重要，逐步增加频率和时间。若某天没有完成，不用补偿性加练，只需在下一个计划点重新开始。\n\n如有心血管、骨关节疾病、孕期或长期未运动，增加活动前可先咨询医生。严重抑郁或有自伤风险时，运动只能作为综合支持的一部分。',
 '数字人心理健康编辑组','WHO抑郁症自我照护建议','["运动","行为激活","自我照护"]',NULL,0,0,0,1,'2026-07-18 09:00:00'),
(9,6,'情绪低落时，怎样向身边的人开口',
 '用具体、可执行的请求代替“你应该懂我”。',
 '求助可以很具体：“我最近两周睡不好、情绪很低，今晚你能陪我吃顿饭吗？”“我想去医院，但一个人有点怕，你能陪我挂号吗？”“你不用马上给建议，先听我说十分钟就好。”\n\n如果对方反应不理想，不代表你的感受不重要。人们可能因为不了解或不知所措而回避。可以换一个人、联系专业人员或拨打心理援助热线。\n\n支持者也可以先问：“你希望我听你说、陪你做点事，还是一起找专业帮助？”若对方提到自伤，不要答应保密；要直接询问当前危险、陪伴并联系专业或紧急资源。',
 '数字人心理健康编辑组','WHO抑郁症自我照护与求助建议','["社会支持","沟通","求助"]',NULL,0,0,0,1,'2026-07-19 09:00:00'),
(10,8,'压力大还是职业倦怠：先看恢复是否有效',
 '识别持续耗竭、疏离和效能下降，并调整可控因素。',
 '短期压力通常在任务结束和休息后缓解；职业倦怠更像持续的精力耗竭、对工作疏离或消极，以及效能感下降。它与工作环境有关，不应全部归咎于个人抗压能力。\n\n先记录一周中的消耗源和恢复源，区分能改变、能协商和暂时不能改变的部分。可尝试明确下班边界、减少非必要切换、安排短休息、与主管协商优先级，并恢复工作外的连接和活动。\n\n倦怠不是医学诊断，但它可能与抑郁、焦虑并存。若低落和兴趣下降扩展到生活各方面，持续至少两周或出现安全风险，应接受专业评估。',
 '数字人心理健康编辑组','WHO职业倦怠概念与心理健康资料','["压力","职业倦怠","边界"]',NULL,0,0,0,1,'2026-07-20 09:00:00'),
(11,3,'惊恐发作时：用慢呼吸和接地回到当下',
 '理解惊恐的身体反应，并使用安全的即时应对步骤。',
 '惊恐发作会带来心跳加快、胸闷、眩晕、发抖和强烈失控感。先确认环境安全，双脚踩地，缓慢呼气；不要反复大口深吸气，以免过度换气加重不适。可以把注意力放到周围：说出看到的5样东西、触到的4种感觉、听到的3个声音。\n\n提醒自己：“这是强烈的焦虑反应，它会过去。”若医生已排除身体急症，可在专业指导下逐步减少回避。\n\n第一次出现严重胸痛、昏厥、呼吸困难，或症状与既往不同，不能自行假定只是惊恐，应及时就医排除身体问题。频繁发作可寻求认知行为治疗等专业帮助。',
 '数字人心理健康编辑组','循证焦虑与惊恐应对原则','["惊恐发作","接地","呼吸"]',NULL,0,0,0,1,'2026-07-21 09:00:00'),
(12,3,'给担忧安排时间：减少全天反复思考',
 '把可解决问题与假设性担忧分开处理。',
 '担忧越想压住，往往越容易反弹。可以每天固定15至20分钟作为“担忧时间”。白天担忧出现时，用一句话记下并告诉自己在固定时间处理。\n\n到时间后，把内容分成两类：有明确行动的问题，写下最小下一步和执行时间；暂时无法行动的假设性担忧，练习允许它存在，同时把注意力带回当下。时间结束后做一个明确的转换动作，如洗脸、散步或整理桌面。\n\n这项练习需要重复，不以完全没有担忧为目标。如果担忧持续数月、难以控制并明显影响睡眠或生活，建议进行专业评估。',
 '数字人心理健康编辑组','认知行为治疗中的担忧管理方法','["担忧","焦虑","CBT"]',NULL,0,0,0,1,'2026-07-22 09:00:00'),
(13,5,'三分钟正念练习：不是清空大脑',
 '用觉察、聚焦和扩展三个步骤练习回到当下。',
 '第一分钟，注意此刻有哪些想法、情绪和身体感觉，只做命名，不急着改变。第二分钟，把注意力收拢到呼吸最明显的位置；走神时，温和地带回来。第三分钟，把觉察扩展到全身、声音和周围空间。\n\n正念不是停止思考，也不是强迫放松。它练习的是发现走神后重新选择注意方向。开始时出现烦躁很常见，可以缩短时间、睁眼练习。\n\n有严重创伤反应、解离或冥想时明显恶化的人，不必勉强闭眼或专注身体，宜在专业人员指导下选择更合适的方法。',
 '数字人心理健康编辑组','WHO压力管理自助资料','["正念","注意力","放松"]',NULL,0,0,0,1,'2026-07-23 09:00:00'),
(14,5,'腹式慢呼吸：重点是呼气舒缓而非吸得更深',
 '一个不追求憋气、不容易过度换气的基础练习。',
 '找一个稳定姿势，一只手放在腹部。用鼻自然吸气约3至4秒，感受腹部轻微起伏；再舒缓呼气约4至6秒。保持轻柔，不必吸到最满，也不要强行屏息。练习1至3分钟后观察身体感受。\n\n如果头晕、胸闷加重，恢复自然呼吸并停止练习。呼吸练习的作用是给注意力一个锚点，并帮助身体逐渐降速，而不是立刻消除所有焦虑。\n\n有呼吸系统或心血管疾病者应按医生建议调整。急性严重胸痛、明显呼吸困难或意识变化需要及时医疗评估。',
 '数字人心理健康编辑组','基础放松训练与安全原则','["呼吸练习","放松","焦虑"]',NULL,0,0,0,1,'2026-07-24 09:00:00'),
(15,7,'自我关怀不是纵容：困难时换一种内在语气',
 '用善意、共同人性和当下觉察回应挫折。',
 '自我关怀包含三个方向：承认“这真的很难”，而不是否认感受；记得挫折是人类共同经验，减少孤立感；用对朋友那样具体而温和的方式回应自己。\n\n可以问：“如果是我在乎的人遇到同样的事，我会怎么说？”再把那句话写给自己。自我关怀不等于取消责任。更完整的表达可以是：“我确实犯了错，也值得在不羞辱自己的情况下修正它。”\n\n如果内在批评与长期创伤、虐待经历或强烈自我厌恶有关，专业治疗能提供更安全的探索空间。',
 '数字人心理健康编辑组','自我关怀与心理治疗通用原则','["自我关怀","自我批评","成长"]',NULL,0,0,0,1,'2026-07-25 09:00:00'),
(16,6,'建立边界：清楚表达能力范围与替代方案',
 '用“事实—感受—需要—请求”减少攻击和含糊。',
 '边界是说明自己会做什么、不会做什么，而不是控制别人。可以说：“今晚我需要休息，不能继续讨论；明天下午三点我可以留出半小时。”这比突然消失或累积到爆发更清楚。\n\n表达时尽量描述具体行为，不给对方贴标签；说明自己的感受和需要；提出可执行的请求或替代方案。对方不一定同意，但你仍可以决定自己的参与方式。\n\n如果关系中存在威胁、跟踪、控制或暴力，普通沟通技巧可能不安全，应优先制定安全计划并联系可信任的人及专业机构。',
 '数字人心理健康编辑组','健康沟通与安全边界原则','["边界","沟通","关系"]',NULL,0,0,0,1,'2026-07-26 09:00:00'),
(17,2,'哀伤没有统一时间表：允许波动，也关注危险信号',
 '理解失落后的自然反应及何时需要额外支持。',
 '失去重要的人、关系、健康或生活角色后，悲伤、愤怒、内疚、麻木和短暂轻松都可能出现。哀伤常呈波浪状，纪念日、地点和气味都可能触发它。没有人人适用的“应该多久走出来”。\n\n维持基本饮食、睡眠和连接，允许以适合自己的方式纪念失去，并避免用酒精或危险行为麻痹感受。支持者不必急着解释或劝人振作，稳定陪伴往往更重要。\n\n若长期几乎无法恢复基本功能，强烈自责或绝望持续加重，或出现自伤想法，应寻求专业评估和危机支持。',
 '数字人心理健康编辑组','WHO哀伤与心理健康通用资料','["哀伤","失落","支持"]',NULL,0,0,0,1,'2026-07-27 09:00:00'),
(18,9,'青少年抑郁可能表现为易怒，而不只是悲伤',
 '家长和照护者应关注持续变化、功能下降和安全线索。',
 '青少年抑郁可能表现为持续易怒、兴趣下降、退缩、成绩骤变、睡眠昼夜颠倒、频繁身体不适、自我评价很低或谈论死亡。单个表现不能说明诊断，关键是与以往相比的持续变化及功能影响。\n\n沟通时先描述观察：“我注意到你最近两周很少出门，也常说自己没用，我有些担心。”少审问、少说教，给出可选择的求助方式。直接询问是否有自伤想法不会“诱导”自杀，反而有助于识别风险。\n\n未成年人出现自伤、明确计划或无法保证安全时，照护者应立即陪伴并联系医疗及紧急资源。',
 '数字人心理健康编辑组','WHO青少年心理健康与抑郁资料','["青少年","抑郁","家长"]',NULL,0,0,1,1,'2026-07-28 09:00:00'),
(19,9,'孕产期抑郁：需要被看见，也可以得到治疗',
 '识别孕期和产后的持续低落、焦虑与功能受损。',
 '孕产期的激素、睡眠、身体变化和照护压力会影响情绪，但持续低落、兴趣下降、强烈焦虑、自责、与婴儿难以建立连接或反复出现伤害自己或婴儿的念头，不应被简单归为“矫情”或“都会这样”。\n\n应主动告诉产科、精神科或心理专业人员当前症状、喂养情况、既往发作及正在使用的药物。治疗方案需要综合评估孕周、哺乳、症状严重度和个人偏好，不要自行停用或开始精神科药物。\n\n如出现幻觉、严重混乱、异常兴奋、几乎不睡仍精力旺盛，或有伤害风险，应按精神科急症立即就医。',
 '数字人心理健康编辑组','WHO孕产妇心理健康资料','["孕产期","产后抑郁","求助"]',NULL,0,0,0,1,'2026-07-29 09:00:00'),
(20,1,'抑郁发作与双相障碍：为什么要询问“兴奋期”',
 '既往躁狂或轻躁狂经历会影响诊断和治疗选择。',
 '有些人在低落之外，曾出现持续数天的异常兴奋或易怒、精力显著增加、睡眠需求减少、话多、想法飞快、自信膨胀或冲动冒险。这些经历可能提示躁狂或轻躁狂，需要在评估抑郁时主动告诉医生。\n\n双相障碍的抑郁期容易被当作单相抑郁，但治疗策略并不相同。不要根据网络测试自行诊断，也不要自行开始、停用或调整抗抑郁药。就诊时可请熟悉自己状态的家人补充时间线。\n\n若出现严重冲动、精神病性症状、连续多日几乎不睡或安全风险，应尽快到精神科或急诊评估。',
 '数字人心理健康编辑组','WHO抑郁症专题中的双相障碍说明','["双相障碍","抑郁发作","鉴别"]',NULL,0,0,1,1,'2026-07-30 09:00:00')
ON DUPLICATE KEY UPDATE category_id=VALUES(category_id), title=VALUES(title), summary=VALUES(summary),
 content=VALUES(content), author=VALUES(author), source=VALUES(source), tags=VALUES(tags),
 is_featured=VALUES(is_featured), is_published=VALUES(is_published), publish_date=VALUES(publish_date);

INSERT INTO psychology_qna
    (qna_id, category_id, question, answer, expert_name, expert_title, tags,
     view_count, like_count, is_verified, status)
VALUES
(1,1,'量表分数高就等于得了抑郁症吗？','不等于。PHQ-9、SDS、CES-D等是筛查或症状监测工具，结果会受近期压力、身体疾病、睡眠、药物等影响。诊断需要专业人员结合访谈、功能影响、病程和其他可能原因综合判断。分数达到筛查界值、症状持续两周以上或明显影响生活时，建议预约精神科、心理科或正规医疗机构评估。','数字人心理健康编辑组','医学内容审核','["量表","诊断","筛查"]',0,0,1,1),
(2,10,'量表中的自伤题选了非零，应该怎么办？','先认真对待，不要只看总分。若已经有具体计划、正在准备工具、难以控制冲动或无法保证安全，请不要独处，远离危险物品，立即联系可信任的人、120/110或心理援助热线12356，并前往急诊。即使没有立即危险，也建议尽快与精神科或心理专业人员讨论并制定安全计划。','数字人心理健康编辑组','危机干预内容审核','["自伤","危机","12356"]',0,0,1,1),
(3,1,'抑郁可以只靠自己调整好吗？','轻微、短暂的情绪困扰可能随支持和生活调整改善，但持续或中重度症状通常需要专业评估。行为激活、规律作息、运动和社会支持可以作为恢复的一部分，却不应成为“必须自己扛”的压力。若功能受损、反复发作、两周以上无改善或有安全风险，应及时求助。','数字人心理健康编辑组','医学内容审核','["抑郁","自助","治疗"]',0,0,1,1),
(4,1,'心理治疗和药物治疗哪个更好？','要看症状严重度、病史、个人偏好、可获得性和安全因素。循证心理治疗和药物都可能有效，中重度抑郁有时会联合使用。药物应由医生评估和随访，不要自行购买、加减或突然停药。可以在就诊时询问预期收益、副作用、替代方案和复诊计划，共同决策。','数字人心理健康编辑组','医学内容审核','["心理治疗","药物","共同决策"]',0,0,1,1),
(5,4,'连续失眠多久需要看医生？','没有唯一时间线。若失眠持续数周、每周多次并影响白天功能，或伴随明显抑郁焦虑、呼吸暂停、异常兴奋、物质使用，应尽早评估。突然出现严重胸闷、意识异常或安全风险需按急症处理。就诊前记录一至两周睡眠日记会有帮助。','数字人心理健康编辑组','睡眠内容审核','["失眠","睡眠日记","就医"]',0,0,1,1),
(6,3,'惊恐发作会不会有生命危险？','惊恐发作本身通常会逐渐缓解，但胸痛、呼吸困难、昏厥等也可能来自身体疾病。首次发作、症状与既往不同、持续不缓解或存在心肺疾病风险时，应及时就医排除急症。确认是惊恐后，可练习慢呼气和接地技术，并寻求专业治疗减少复发和回避。','数字人心理健康编辑组','医学内容审核','["惊恐发作","急症","焦虑"]',0,0,1,1),
(7,6,'朋友说“不想活了”，我怕问多了会刺激他吗？','直接、平静地询问是否想到自伤、是否有计划和是否能保证当前安全，不会把自杀念头“塞给”对方。倾听、不批评、不答应保密；若有具体计划、工具或立即危险，陪着对方并联系120/110、12356、家属或医疗机构。支持者也可以寻求专业指导，不要独自承担。','数字人心理健康编辑组','危机干预内容审核','["朋友支持","自杀预防","倾听"]',0,0,1,1),
(8,7,'没有动力时，计划总完不成怎么办？','把任务缩小到几乎不可能失败，例如不是“锻炼一小时”，而是“穿鞋下楼五分钟”；指定时间和地点，完成后记录而不评价。计划失败时检查门槛是否仍太高、是否需要他人陪伴。若连吃饭、洗漱等基本活动都很困难，或持续恶化，应尽快寻求专业帮助。','数字人心理健康编辑组','行为激活内容审核','["动力","行为激活","小目标"]',0,0,1,1),
(9,5,'正念练习时反而更焦虑，是我做错了吗？','不一定。安静下来后更注意到身体或想法，可能让焦虑暂时增强。可以睁眼、缩短到30秒，把注意力放到外部声音或脚底触地感，不必强迫自己继续。若有创伤、解离，或练习反复显著恶化，请暂停并与专业人员讨论替代方法。','数字人心理健康编辑组','正念安全内容审核','["正念","焦虑","创伤知情"]',0,0,1,1),
(10,10,'12356是什么热线？','12356是国家统一心理援助热线短号码，用于提供心理咨询、心理疏导、危机干预和必要转介。它适合在情绪困扰或危机时寻求支持；若已经发生严重伤害、存在立即生命危险或需要现场医疗救治，应同时拨打120/110或直接前往急诊。','数字人心理健康编辑组','国家卫健委公开资料','["12356","心理援助","热线"]',0,0,1,1)
ON DUPLICATE KEY UPDATE category_id=VALUES(category_id), question=VALUES(question), answer=VALUES(answer),
 expert_name=VALUES(expert_name), expert_title=VALUES(expert_title), tags=VALUES(tags),
 is_verified=VALUES(is_verified), status=VALUES(status);

INSERT INTO psychology_resources
    (resource_id, category_id, resource_type, title, description, file_data, external_url,
     file_size, mime_type, duration, thumbnail, tags, view_count, like_count, status)
VALUES
(1,1,'LINK','WHO：抑郁症专题','世界卫生组织关于抑郁症表现、风险因素、治疗和自我照护的权威概览。',NULL,'https://www.who.int/health-topics/depression',NULL,'text/html',NULL,NULL,'["WHO","抑郁症","权威科普"]',0,0,1),
(2,5,'PDF','WHO：重要之事，压力时期的应对指南','世界卫生组织面向公众的循证压力管理自助指南，包含接地、脱钩、价值行动等练习。',NULL,'https://www.who.int/publications/i/item/9789240003927',NULL,'text/html',NULL,NULL,'["WHO","压力管理","自助指南"]',0,0,1),
(3,1,'LINK','NIMH：Depression','美国国家精神卫生研究所的抑郁症症状、风险与治疗信息。',NULL,'https://www.nimh.nih.gov/health/topics/depression',NULL,'text/html',NULL,NULL,'["NIMH","抑郁症","治疗"]',0,0,1),
(4,10,'LINK','国家卫健委：全国统一心理援助热线12356','国家卫生健康委关于推进全国统一心理援助热线12356应用的公开说明。',NULL,'https://www.gov.cn/zhengce/zhengceku/202502/content_7003644.htm',NULL,'text/html',NULL,NULL,'["12356","心理援助","国家卫健委"]',0,0,1),
(5,9,'LINK','WHO：青少年心理健康','关于青少年常见心理健康问题、风险因素和支持方式的权威信息。',NULL,'https://www.who.int/news-room/fact-sheets/detail/adolescent-mental-health',NULL,'text/html',NULL,NULL,'["WHO","青少年","心理健康"]',0,0,1),
(6,10,'LINK','WHO：自杀预防','世界卫生组织关于自杀风险、预防和公共卫生行动的信息。',NULL,'https://www.who.int/health-topics/suicide',NULL,'text/html',NULL,NULL,'["WHO","自杀预防","危机"]',0,0,1),
(7,4,'LINK','NHLBI：健康睡眠指南','美国国家心肺血液研究所关于睡眠不足和健康睡眠习惯的公众资料。',NULL,'https://www.nhlbi.nih.gov/health/sleep-deprivation',NULL,'text/html',NULL,NULL,'["睡眠","NHLBI","睡眠卫生"]',0,0,1),
(8,7,'LINK','WHO：身体活动专题','世界卫生组织关于身体活动对整体健康益处及建议的资料。',NULL,'https://www.who.int/health-topics/physical-activity',NULL,'text/html',NULL,NULL,'["WHO","身体活动","健康"]',0,0,1)
ON DUPLICATE KEY UPDATE category_id=VALUES(category_id), resource_type=VALUES(resource_type),
 title=VALUES(title), description=VALUES(description), external_url=VALUES(external_url),
 mime_type=VALUES(mime_type), tags=VALUES(tags), status=VALUES(status);

COMMIT;
