-- 初始化数字陪伴系统样例数据
-- 执行前请确保已经创建基础表结构（参见 create-db-template.sql）

USE
digital_companion;

SET NAMES utf8mb4;
SET
FOREIGN_KEY_CHECKS = 0;

TRUNCATE TABLE community_comments;
TRUNCATE TABLE community_post_media;
TRUNCATE TABLE community_posts;
TRUNCATE TABLE depression_assessments;
TRUNCATE TABLE depression_scales;
TRUNCATE TABLE conversation_messages;
TRUNCATE TABLE conversations;
TRUNCATE TABLE user_diaries;
TRUNCATE TABLE user_profiles;
TRUNCATE TABLE users;

SET
FOREIGN_KEY_CHECKS = 1;

-- 基础用户
-- 密码: 123123123 (BCrypt加密存储)
INSERT INTO users (id, username, password, email, phone, avatar, nickname, created_at, updated_at, last_login_at,
                   status)
VALUES (1, 'alice', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm', 'alice@example.com', '13800000001',
        NULL, '清晨小太阳', '2025-10-01 08:30:00', '2025-10-18 10:12:00', '2025-10-18 10:12:00', 1),
       (2, 'bruce', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm', 'bruce@example.com', '13800000002',
        NULL, '海边散步者', '2025-10-01 09:00:00', '2025-10-18 12:05:00', '2025-10-18 12:05:00', 1),
       (3, 'chloe', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm', 'chloe@example.com', '13800000003',
        NULL, '慢生活研究员', '2025-10-02 07:55:00', '2025-10-18 08:45:00', '2025-10-18 08:45:00', 1),
       (4, 'dylan', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm', 'dylan@example.com', '13800000004',
        NULL, '森林里的猫', '2025-10-02 10:25:00', '2025-10-18 21:02:00', '2025-10-18 21:02:00', 1),
       (5, 'elena', '$2a$10$ayH0oUppUHYfMg4BAwgx1OBrDx7hXRYqM8Dz8iMbiJXaVpWmyEgrm', 'elena@example.com', '13800000005',
        NULL, '夜空观星人', '2025-10-03 06:40:00', '2025-10-18 22:18:00', '2025-10-18 22:18:00', 1);

-- 用户画像
INSERT INTO user_profiles (id, user_id, interests, personality_traits, interaction_preferences, emotional_tendency,
                           learning_records, created_at, updated_at)
VALUES (1, 1, '["晨间瑜伽","手帐","规律作息"]', '["敏感细腻","INFJ"]', '["温柔鼓励","健康习惯","每日签到"]',
        '["平和","睡眠不佳","呼吸练习"]', '[]', '2025-10-18 09:30:00', '2025-10-18 10:05:00'),
       (2, 2, '["海边散步","摄影","保持运动"]', '["务实","ISTJ"]', '["直接建议","运动计划"]',
        '["坚韧","手心出汗","握拳放松"]', '[]', '2025-10-18 11:00:00', '2025-10-18 11:42:00'),
       (3, 3, '["慢跑","手工","提升专注"]', '["乐观","ENFP"]', '["陪伴式","创意活动","每日签到"]',
        '["积极","注意力涣散","冥想"]', '[]', '2025-10-18 13:15:00', '2025-10-18 13:40:00'),
       (4, 4, '["森林散步","整理空间"]', '["沉稳","ISFP"]', '["温柔倾听","自我关怀"]', '["温厚","情绪堆积","写信"]',
        '[]', '2025-10-18 15:20:00', '2025-10-18 16:05:00'),
       (5, 5, '["观星","夜跑","改善睡眠"]', '["好奇","INFP"]', '["启发式","睡前放松","每日签到"]',
        '["温柔","入睡困难","热水澡"]', '[]', '2025-10-18 18:10:00', '2025-10-18 18:45:00');

-- 抑郁量表定义
INSERT INTO depression_scales (scale_id, scale_name, scale_description, min_score, max_score, severity_ranges,
                               questions, created_at, updated_at)
VALUES (1, 'PHQ-9', '患者健康问卷抑郁量表（PHQ-9），用于评估过去两周的抑郁程度，9 题累计 0-27 分。', 0, 27,
        '[{"range":"0-4","label":"无或最小程度抑郁"},{"range":"5-9","label":"轻度抑郁"},{"range":"10-14","label":"中度抑郁"},{"range":"15-19","label":"中重度抑郁"},{"range":"20-27","label":"重度抑郁"}]',
        '[{"id":1,"text":"对做事情缺乏兴趣或乐趣","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":2,"text":"感到情绪低落、沮丧或无望","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":3,"text":"难以入睡、睡眠不佳或睡得过多","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":4,"text":"感到疲乏或精力不足","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":5,"text":"食欲不振或过度饮食","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":6,"text":"对自己感到不满，觉得自己失败或让家人失望","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":7,"text":"难以专注于做事","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":8,"text":"动作或讲话变慢，或者烦躁不安","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]},{"id":9,"text":"觉得活着没有意义、或想到伤害自己","options":[{"score":0,"label":"完全没有"},{"score":1,"label":"几天"},{"score":2,"label":"超过一半天"},{"score":3,"label":"几乎每天"}]}]',
        '2025-10-18 09:00:00', '2025-10-18 09:00:00'),
       (2, 'SDS', '自评抑郁量表（SDS），采用 4 级频率评分，原始分 20-80。', 20, 80,
        '[{"range":"20-39","label":"无抑郁"},{"range":"40-49","label":"轻度抑郁"},{"range":"50-59","label":"中度抑郁"},{"range":"60-80","label":"重度抑郁"}]',
        '[{"id":1,"text":"我觉得闷闷不乐，情绪低沉","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":2,"text":"我仍旧像往常一样享受生活中的乐趣","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]},{"id":3,"text":"我忽然觉得要哭了","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":4,"text":"我晚上睡眠不好","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":5,"text":"我吃饭和往常一样多","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]},{"id":6,"text":"我的心跳比平时快","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":7,"text":"我平时做事慢吞吞","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":8,"text":"我对未来充满信心","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]},{"id":9,"text":"我觉得比平时容易疲劳","options":[{"score":1,"label":"很少"},{"score":2,"label":"有时"},{"score":3,"label":"经常"},{"score":4,"label":"持续"}]},{"id":10,"text":"我的头脑依然像往常一样清楚","options":[{"score":4,"label":"持续"},{"score":3,"label":"经常"},{"score":2,"label":"有时"},{"score":1,"label":"很少"}]}]',
        '2025-10-18 09:05:00', '2025-10-18 09:05:00'),
       (3, 'BDI-II', '贝克抑郁量表第二版（BDI-II），每题 0-3 级别描述，累计 0-63 分。', 0, 63,
        '[{"range":"0-13","label":"最小抑郁"},{"range":"14-19","label":"轻度抑郁"},{"range":"20-28","label":"中度抑郁"},{"range":"29-63","label":"重度抑郁"}]',
        '[{"id":1,"text":"悲伤程度","options":[{"score":0,"label":"没有特别悲伤"},{"score":1,"label":"偶尔有些悲伤"},{"score":2,"label":"经常感到悲伤"},{"score":3,"label":"持续深度悲伤"}]},{"id":2,"text":"对未来的悲观","options":[{"score":0,"label":"对未来乐观"},{"score":1,"label":"有时担心未来"},{"score":2,"label":"觉得未来黯淡"},{"score":3,"label":"确信未来绝望"}]},{"id":3,"text":"失败感","options":[{"score":0,"label":"不觉得失败"},{"score":1,"label":"有时觉得失败"},{"score":2,"label":"常感失败"},{"score":3,"label":"一直觉得完全失败"}]},{"id":4,"text":"丧失快乐能力","options":[{"score":0,"label":"仍能享受活动"},{"score":1,"label":"快乐感下降"},{"score":2,"label":"难以从活动中获得快乐"},{"score":3,"label":"完全失去快乐感"}]},{"id":5,"text":"罪恶感","options":[{"score":0,"label":"几乎没有罪恶感"},{"score":1,"label":"偶尔感到内疚"},{"score":2,"label":"经常觉得自己不好"},{"score":3,"label":"持续觉得自己糟糕"}]},{"id":6,"text":"惩罚感","options":[{"score":0,"label":"不觉得该受罚"},{"score":1,"label":"有时觉得该受罚"},{"score":2,"label":"常常觉得该受罚"},{"score":3,"label":"确信自己应受惩罚"}]},{"id":7,"text":"不喜欢自己","options":[{"score":0,"label":"喜欢自己"},{"score":1,"label":"有时不喜欢自己"},{"score":2,"label":"经常不喜欢自己"},{"score":3,"label":"完全讨厌自己"}]},{"id":8,"text":"自我批评","options":[{"score":0,"label":"与他人一样优秀"},{"score":1,"label":"对自己严格"},{"score":2,"label":"常自我批评"},{"score":3,"label":"持续自我指责"}]},{"id":9,"text":"自杀念头","options":[{"score":0,"label":"没有想法"},{"score":1,"label":"偶尔闪过想法"},{"score":2,"label":"常想到自伤"},{"score":3,"label":"已有计划或行动"}]},{"id":10,"text":"哭泣频率","options":[{"score":0,"label":"与平时相同"},{"score":1,"label":"哭得更多"},{"score":2,"label":"几乎每天哭"},{"score":3,"label":"想哭却哭不出来"}]},{"id":11,"text":"易怒程度","options":[{"score":0,"label":"不比平时易怒"},{"score":1,"label":"稍微易怒"},{"score":2,"label":"常常易怒"},{"score":3,"label":"持续怒火难控"}]},{"id":12,"text":"社交退缩","options":[{"score":0,"label":"愿意与人交往"},{"score":1,"label":"较少社交"},{"score":2,"label":"常回避社交"},{"score":3,"label":"完全不与人交往"}]},{"id":13,"text":"优柔寡断","options":[{"score":0,"label":"决策正常"},{"score":1,"label":"决策变慢"},{"score":2,"label":"难以下决定"},{"score":3,"label":"几乎无法决定任何事"}]},{"id":14,"text":"无价值感","options":[{"score":0,"label":"感觉自己有价值"},{"score":1,"label":"偶尔怀疑价值"},{"score":2,"label":"常觉得自己无价值"},{"score":3,"label":"确信自己毫无价值"}]},{"id":15,"text":"精力水平","options":[{"score":0,"label":"精力正常"},{"score":1,"label":"精力下降"},{"score":2,"label":"精力很低"},{"score":3,"label":"几乎无精力"}]},{"id":16,"text":"睡眠变化","options":[{"score":0,"label":"睡眠正常"},{"score":1,"label":"入睡稍有困难"},{"score":2,"label":"严重失眠或嗜睡"},{"score":3,"label":"几乎无法维持睡眠"}]},{"id":17,"text":"疲劳程度","options":[{"score":0,"label":"不比平时疲劳"},{"score":1,"label":"稍感疲劳"},{"score":2,"label":"常感疲劳"},{"score":3,"label":"极度疲劳难以活动"}]},{"id":18,"text":"食欲变化","options":[{"score":0,"label":"食欲正常"},{"score":1,"label":"食欲略降或略增"},{"score":2,"label":"明显变化"},{"score":3,"label":"几乎不能进食或控制进食"}]},{"id":19,"text":"体重变化","options":[{"score":0,"label":"体重稳定"},{"score":1,"label":"轻微变化"},{"score":2,"label":"明显变化"},{"score":3,"label":"体重大幅波动"}]},{"id":20,"text":"性欲下降","options":[{"score":0,"label":"性欲正常"},{"score":1,"label":"性欲略降"},{"score":2,"label":"性欲明显下降"},{"score":3,"label":"完全没有性欲"}]}]',
        '2025-10-18 09:10:00', '2025-10-18 09:10:00');

-- 用户与数字陪伴对话（会话元数据）
#
INSERT INTO conversations (id, user_id, title, is_title_generated, last_message_at, message_count, created_at)
# VALUES
# 	(1, 1, '夜间情绪疏导对话', 1, '2025-10-17 21:55:05', 4, '2025-10-17 21:55:30'),
# 	(2, 1, '晨间目标检视', 0, '2025-10-18 07:14:05', 4, '2025-10-18 07:15:00');

-- 社区帖子
INSERT INTO community_posts (post_id, user_id, title, content, extra_metadata, likes_count, comments_count, status,
                             created_at, updated_at)
VALUES (1, 1, '晨间呼吸练习分享', '今天按照应用的呼吸引导练了十分钟，胸口的压抑感缓解了不少。',
        '{"tags":["breath","morning"],"mood":"relaxed"}', 18, 2, 1, '2025-10-12 07:20:00', '2025-10-12 08:15:00'),
       (2, 1, '周末小目标', '计划完成两段散步和一次手帐记录，有一起的吗？',
        '{"tags":["plan","weekend"],"mood":"hopeful"}', 12, 2, 1, '2025-10-13 09:05:00', '2025-10-13 11:10:00'),
       (3, 1, '音乐疗愈歌单', '搜集了一些温柔的钢琴曲，放松睡前的紧绷。欢迎补充。',
        '{"tags":["music","sleep"],"mood":"calm"}', 25, 2, 1, '2025-10-14 21:22:00', '2025-10-15 08:00:00'),
       (4, 2, '今天的步行记录', '傍晚在海边走了3500步，风很大但心情轻松了。',
        '{"tags":["walk","evening"],"mood":"energized"}', 20, 2, 1, '2025-10-11 18:40:00', '2025-10-11 19:30:00'),
       (5, 2, '焦虑时的小动作', '分享一个握拳再放松的动作，配合呼吸能让手心没那么出汗。',
        '{"tags":["anxiety","tips"],"mood":"steady"}', 16, 2, 1, '2025-10-15 10:15:00', '2025-10-15 12:32:00'),
       (6, 2, '第一次尝试冥想', '坐了十五分钟还是会走神，不过结束后脑袋清亮了点。',
        '{"tags":["meditation"],"mood":"curious"}', 14, 2, 1, '2025-10-16 07:45:00', '2025-10-16 08:20:00'),
       (7, 3, '慢跑的呼吸节奏', '医生建议的节奏是吸两步呼两步，第一次觉得自己能坚持。',
        '{"tags":["run","breath"],"mood":"motivated"}', 22, 2, 1, '2025-10-10 06:30:00', '2025-10-10 07:00:00'),
       (8, 3, '手工疗愈时间', '做了一只小狐狸布偶，手忙脚乱但很治愈。',
        '{"tags":["craft","mindfulness"],"mood":"focused"}', 19, 2, 1, '2025-10-14 14:05:00', '2025-10-14 15:40:00'),
       (9, 3, '饮食记录', '今天尝试了新的燕麦酸奶搭配，口感不错也很顶饱。',
        '{"tags":["diet","breakfast"],"mood":"content"}', 11, 2, 1, '2025-10-15 08:10:00', '2025-10-15 09:05:00'),
       (10, 3, '正念时刻', '午餐前闭眼一分钟感受味道，让自己慢一点。',
        '{"tags":["mindfulness","meal"],"mood":"peaceful"}', 13, 2, 1, '2025-10-16 12:00:00', '2025-10-16 12:20:00'),
       (11, 4, '森林散步日记', '早晨在林间绕了一圈，阳光透过叶子像星星。', '{"tags":["nature","walk"],"mood":"grounded"}',
        28, 2, 1, '2025-10-09 07:10:00', '2025-10-09 08:05:00'),
       (12, 4, '写给过去的自己', '给三年前的自己写了一封信，提醒那时也很勇敢。',
        '{"tags":["journaling","self-compassion"],"mood":"reflective"}', 24, 2, 1, '2025-10-13 22:18:00',
        '2025-10-14 06:50:00'),
       (13, 4, '开窗整理房间', '整理柜子的时候发现了好多旧票根，原来快乐的瞬间不止一个。',
        '{"tags":["declutter","memory"],"mood":"nostalgic"}', 17, 2, 1, '2025-10-15 16:40:00', '2025-10-15 17:05:00'),
       (14, 4, '猫咪陪伴', '猫咪坐在腿上打呼噜的那刻，世界好像没那么吵。', '{"tags":["pet","warmth"],"mood":"soothed"}',
        35, 2, 1, '2025-10-17 21:15:00', '2025-10-17 21:40:00'),
       (15, 5, '星空观察记录', '昨晚看到了猎户座的形状，拍了一张糊糊的照片。',
        '{"tags":["stargazing","night"],"mood":"awed"}', 30, 2, 1, '2025-10-11 23:05:00', '2025-10-12 00:10:00'),
       (16, 5, '睡前放松流程', '热水澡+薰衣草精油+轻柔音乐，睡前焦虑缓解不少。',
        '{"tags":["sleep","routine"],"mood":"sleepy"}', 21, 2, 1, '2025-10-13 23:20:00', '2025-10-14 00:05:00'),
       (17, 5, '感恩练习清单', '写下了今天感谢的三件小事，感觉心里亮了。', '{"tags":["gratitude"],"mood":"grateful"}', 18,
        2, 1, '2025-10-15 21:30:00', '2025-10-15 22:00:00'),
       (18, 5, '夜跑的节奏', '夜跑配着喜欢的节奏感音乐，心跳稳了一些。', '{"tags":["run","night"],"mood":"balanced"}', 23,
        2, 1, '2025-10-16 20:40:00', '2025-10-16 21:30:00'),
       (19, 2, '线上互助小组体验', '第一次参加线上互助小组，感受到大家彼此的照亮。',
        '{"tags":["support-group"],"mood":"supported"}', 27, 2, 1, '2025-10-18 15:10:00', '2025-10-18 16:00:00'),
       (20, 3, '心情颜色打卡', '今天给自己的心情涂成了浅绿色，有新的生长。',
        '{"tags":["mood-tracking"],"mood":"hopeful"}', 15, 2, 1, '2025-10-18 18:25:00', '2025-10-18 18:50:00');

-- 社区帖子媒体资源
-- 使用Python脚本生成的真实JPEG图片数据
INSERT INTO community_post_media (media_id, post_id, media_type, mime_type, media_data, created_at)
VALUES (1, 1, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/tsH/2Q=='),
        '2025-10-12 07:21:00'),
       (2, 3, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwCt2Ob/2Q=='),
        '2025-10-14 21:30:00'),
       (3, 4, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwBI0cz/2Q=='),
        '2025-10-11 18:45:00'),
       (4, 5, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/1wD/2Q=='),
        '2025-10-15 10:20:00'),
       (5, 7, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/jAD/2Q=='),
        '2025-10-10 06:35:00'),
       (6, 8, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/wMv/2Q=='),
        '2025-10-14 14:10:00'),
       (7, 11, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwCQ7pD/2Q=='),
        '2025-10-09 07:15:00'),
       (8, 14, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwDdoN3/2Q=='),
        '2025-10-17 21:20:00'),
       (9, 15, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwAZGXD/2Q=='),
        '2025-10-11 23:10:00'),
       (10, 16, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwDm5vr/2Q=='),
        '2025-10-13 23:25:00'),
       (11, 18, 'IMAGE', 'image/jpeg',
        FROM_BASE64('/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAAAP/aAAgBAQAAPwD/oHr/2Q=='),
        '2025-10-16 20:45:00');

-- 社区评论（每个帖子 2 条，含一条回复）
INSERT INTO community_comments (comment_id, post_id, user_id, parent_comment_id, content, attachments, likes_count,
                                status, created_at, updated_at)
VALUES (1, 1, 2, NULL, '听起来好棒，我也准备明早跟练一次看看效果。', '[]', 6, 1, '2025-10-12 08:10:00',
        '2025-10-12 08:10:00'),
       (2, 1, 1, 1, '一起加油，记得结束后做个伸展更舒服。', '[]', 4, 1, '2025-10-12 08:14:00', '2025-10-12 08:14:00'),
       (3, 2, 3, NULL, '我可以加入手帐计划，周末互相打卡吧！', '[]', 5, 1, '2025-10-13 11:20:00', '2025-10-13 11:20:00'),
       (4, 2, 1, 3, '好呀，我创建了共享任务列表，私信你。', '[]', 3, 1, '2025-10-13 11:28:00', '2025-10-13 11:28:00'),
       (5, 3, 4, NULL, '感谢分享，我这周正好在找助眠的钢琴曲。', '[]', 7, 1, '2025-10-15 08:10:00',
        '2025-10-15 08:10:00'),
       (6, 3, 1, 5, '如果你喜欢轻柔的，可以试试月光奏鸣曲慢板。', '[]', 4, 1, '2025-10-15 08:12:00',
        '2025-10-15 08:12:00'),
       (7, 4, 5, NULL, '海风真的很提神，记得带保暖哦。', '[]', 6, 1, '2025-10-11 19:20:00', '2025-10-11 19:20:00'),
       (8, 4, 2, 7, '收到！下次准备一个围巾，感谢提醒。', '[]', 2, 1, '2025-10-11 19:24:00', '2025-10-11 19:24:00'),
       (9, 5, 1, NULL, '这个动作我也在用，配合慢数很有效。', '[]', 5, 1, '2025-10-15 12:40:00', '2025-10-15 12:40:00'),
       (10, 5, 2, 9, '太好了，我们多交流缓解焦虑的小技巧。', '[]', 3, 1, '2025-10-15 12:45:00', '2025-10-15 12:45:00'),
       (11, 6, 3, NULL, '走神很正常，我会轻声提醒自己回到呼吸上。', '[]', 4, 1, '2025-10-16 08:30:00',
        '2025-10-16 08:30:00'),
       (12, 6, 2, 11, '谢谢提示，下次试试专注在空气进出的感觉。', '[]', 2, 1, '2025-10-16 08:34:00',
        '2025-10-16 08:34:00'),
       (13, 7, 4, NULL, '节奏很稳！建议热身时加上脚踝放松。', '[]', 6, 1, '2025-10-10 07:05:00', '2025-10-10 07:05:00'),
       (14, 7, 3, 13, '好建议，我之前老忘记，等下就补上。', '[]', 3, 1, '2025-10-10 07:08:00', '2025-10-10 07:08:00'),
       (15, 8, 5, NULL, '小狐狸好可爱，能分享纸型来源吗？', '[]', 4, 1, '2025-10-14 15:45:00', '2025-10-14 15:45:00'),
       (16, 8, 3, 15, '来自一个免费模板网站，回头把链接贴上。', '[]', 2, 1, '2025-10-14 15:50:00', '2025-10-14 15:50:00'),
       (17, 9, 2, NULL, '燕麦酸奶加点草莓干也不错！', '[]', 3, 1, '2025-10-15 09:10:00', '2025-10-15 09:10:00'),
       (18, 9, 3, 17, '听起来很赞，晚上去买草莓干！', '[]', 1, 1, '2025-10-15 09:12:00', '2025-10-15 09:12:00'),
       (19, 10, 1, NULL, '正念进餐真的能让胃舒服不少。', '[]', 4, 1, '2025-10-16 12:25:00', '2025-10-16 12:25:00'),
       (20, 10, 3, 19, '是的，还能更快察觉饱腹感。', '[]', 2, 1, '2025-10-16 12:28:00', '2025-10-16 12:28:00'),
       (21, 11, 1, NULL, '阳光穿过树叶的样子想象出来就好治愈。', '[]', 5, 1, '2025-10-09 08:10:00',
        '2025-10-09 08:10:00'),
       (22, 11, 4, 21, '欢迎哪天一起散步，分享更多光影瞬间。', '[]', 3, 1, '2025-10-09 08:14:00', '2025-10-09 08:14:00'),
       (23, 12, 5, NULL, '好喜欢这种写信的方式，也想试着写给那个时候的自己。', '[]', 7, 1, '2025-10-14 07:10:00',
        '2025-10-14 07:10:00'),
       (24, 12, 4, 23, '写完真的会更温柔地看待自己，推荐试试。', '[]', 3, 1, '2025-10-14 07:15:00',
        '2025-10-14 07:15:00'),
       (25, 13, 2, NULL, '旧票根一定装着好多故事，期待你分享。', '[]', 4, 1, '2025-10-15 17:10:00',
        '2025-10-15 17:10:00'),
       (26, 13, 4, 25, '改天整理成帖子，分享给大家。', '[]', 2, 1, '2025-10-15 17:14:00', '2025-10-15 17:14:00'),
       (27, 14, 3, NULL, '猫猫的陪伴真的能缓解很多焦虑。', '[]', 6, 1, '2025-10-17 21:45:00', '2025-10-17 21:45:00'),
       (28, 14, 4, 27, '是啊，它总能在我最需要的时候靠过来。', '[]', 3, 1, '2025-10-17 21:49:00', '2025-10-17 21:49:00'),
       (29, 15, 2, NULL, '星空照片好美！昨晚的云层终于散开了。', '[]', 8, 1, '2025-10-12 00:20:00',
        '2025-10-12 00:20:00'),
       (30, 15, 5, 29, '下次一起观星吧，我再带一台双筒望远镜。', '[]', 4, 1, '2025-10-12 00:25:00',
        '2025-10-12 00:25:00'),
       (31, 16, 1, NULL, '薰衣草精油真的很助眠，我也常用。', '[]', 5, 1, '2025-10-14 00:15:00', '2025-10-14 00:15:00'),
       (32, 16, 5, 31, '谢谢认同，我也在尝试加入舒缓伸展。', '[]', 2, 1, '2025-10-14 00:18:00', '2025-10-14 00:18:00'),
       (33, 17, 3, NULL, '感恩清单写完会觉得内心变柔软。', '[]', 4, 1, '2025-10-15 22:05:00', '2025-10-15 22:05:00'),
       (34, 17, 5, 33, '是的，而且能慢慢记录生活闪光点。', '[]', 2, 1, '2025-10-15 22:08:00', '2025-10-15 22:08:00'),
       (35, 18, 1, NULL, '夜跑注意脚步安全，我通常带个反光臂带。', '[]', 6, 1, '2025-10-16 21:40:00',
        '2025-10-16 21:40:00'),
       (36, 18, 5, 35, '好主意，今晚就加上，谢谢提醒！', '[]', 3, 1, '2025-10-16 21:45:00', '2025-10-16 21:45:00'),
       (37, 19, 4, NULL, '互助小组的氛围听起来好正向，想了解如何报名。', '[]', 5, 1, '2025-10-18 16:05:00',
        '2025-10-18 16:05:00'),
       (38, 19, 2, 37, '后台发链接给你了，也欢迎你分享经验。', '[]', 3, 1, '2025-10-18 16:10:00', '2025-10-18 16:10:00'),
       (39, 20, 1, NULL, '浅绿色真有生命力，祝我们都有新芽。', '[]', 4, 1, '2025-10-18 19:00:00', '2025-10-18 19:00:00'),
       (40, 20, 3, 39, '谢谢祝福，一起继续记录心情颜色。', '[]', 2, 1, '2025-10-18 19:05:00', '2025-10-18 19:05:00');
