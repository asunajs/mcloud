import Utils from "../common/utils";
import Tools from "../common/tools";
import apiNative from "./apiNative";
import { getCacheDataFn } from "./cacheReq";
import { AdvSignDemo, AESUtilDemo } from "./AdconfigSign";
import store from '../../login/src/store.js'; // 根据实际路径调整
// SIM登录相关
let countDown = 60;
let simResultTimer = null;
const isMacPc = navigator.userAgent.toLowerCase().indexOf('mac') !== -1
let clientType = isMacPc ? '710' : '656';
let cpid = isMacPc ? '332' : '277';
// 授权二维码登录
let authQRCodeTimer = null;

// 账号密码登录，定义枚举结构
const LOGIN_PROTECT_TYPE = {
  NO_LOGIN_PROTECT: 0,
  PWD_LOGIN_PROTECT: 1, // 账密登陆保护
  REGION_LOGIN_PROTECT: 2, // 异地登录保护
};

// 账号密码登录, 安全登录的recordId
var protectRecordId = "";

function isCutOver() {
  return UserInfo.cutoverStatus == 1 ? true : false;
}

export default {
  common: {
    async getPreviewInfo(params,ismountdisk = false) {
      //获取媒体(音视频)文件在线播放地址
      let requestUrl = "/hcy/videoPreview/getPreviewInfo";
      if(ismountdisk){
        requestUrl = "/mount/videoPreview/getPreviewInfo";
      }
      let options = {
        type: "json",
      };
      if (params.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete params.isSafeBox;
      }
      if (params.category == "video" && ismountdisk == false ) { //挂载盘不进行异网检测
        let enable = await apiNative.global.getNetworkOptConfig();
        let isNonLocal = await apiNative.global.isNonLocalNetwork(Tools.getPublicUrl("ip-info") || "");
        console.log('异网开关enable：', enable, '异网检测isNonLocal：', isNonLocal);
        if (enable && isNonLocal) {
          if (!options.appendHeads) {
            options.appendHeads = {};
          }
          options.appendHeads["x-yun-url-type"] = 3;
        }
      }
      return Utils.sendRequest(requestUrl, params, options);
    },
    async getMoviePreviewInfo(params) {
      //获取影视资源文件在线播放地址
      let requestUrl = "/orchestration/file/video/v1.0/queryFlvAddr";
      let options = {
        type: "json",
      };
      if (params.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete params.isSafeBox;
      }
      if (params.category == "video") {
        let enable = await apiNative.global.getNetworkOptConfig();
        let isNonLocal = await apiNative.global.isNonLocalNetwork(Tools.getPublicUrl("ip-info") || "");
        console.log('异网开关enable：', enable, '异网检测isNonLocal：', isNonLocal);
        if (enable && isNonLocal) {
          if (!options.appendHeads) {
            options.appendHeads = {};
          }
          options.appendHeads["x-yun-url-type"] = 3;
        }
      }
      return Utils.sendRequest(requestUrl, params, options);
    },
    //获取公钥
    getRsaPublicKey(data) {
      var requestUrl = "/caiyun/openapi/authentication/key/getRsaPublicKey";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    //获取视频进度
    getVideoProgress(data) {
      var requestUrl = "/openapi/pDynamicInfo/pagePersonalDynamicMedia";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    //获取视频进度
    getVideoProgress(data) {
      var requestUrl = "/openapi/pDynamicInfo/pagePersonalDynamicMedia";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    //获取音频进度
    getAudioProgress(data) {
      var requestUrl = "/openapi/pDynamicInfo/pagePersonalDynamicAudio";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 上报用户最近动态
    dynamicDeclare(contentId, contentType,extraData) {
      let requestUrl = "/openapi/pDynamicInfo/uploadBrowseData";
      let obj={contentId,contentType}
      let data = {
        userId: window.userDomainId || window.UserInfo.userDomainId || '',
        userDomainId: window.userDomainId || window.UserInfo.userDomainId || '',
        terminalType: 5,
        dynamicContentList: [obj],
      };
      if(extraData && [2,3].includes(extraData.contentType)){
        obj.contentSchedule=extraData.contentSchedule+''
        obj.time=new Date().getTime()
        obj.parentCatalogID=extraData.parentCatalogID
        obj.ending=extraData.ending || 0
        obj.contentType=extraData.contentType
        data.dynamicType=1
        data.encodeType=2
        data.dynamicContentList=[obj]
      }
      let signatureData = Utils.createSignature();
      let options = {
        type: "json",
        appendHeads: {
          "clientId": signatureData.clientId,
          "msgId": signatureData.msgId,
          "signature": signatureData.hash,
          "timestamp": signatureData.timeStamp,
          "version": signatureData.version,
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 仅个人云和同步盘的文件预览允许上报最近浏览记录
    shouldUploadBrowseData(source) {
      return ["personalCloud", "asyncTask"].includes(source);
    },
  },
  auth: {
    /* 退出登录 */
    quitOutLogin() {
      let data = {
        phoneNumber: window.userAccount || window.UserInfo.account,
      };
      return new Promise((resolve) => {
        Utils.sendRequest("/user/logout", data, { type: "json" }).then((result) => {
          let obj = Utils.changeResultData(result);
          resolve(obj);
        });
      });
    },
    /* 刷新token */
    refreshToken(data) {
      return new Promise((resolve) => {
        Utils.sendRequest("/user/auth/refreshToken", data,{ type: "json" }).then((result) => {
          console.log('刷新token',result)
          resolve(result);
        });
      });
    },
    /* 发送验证码至用户 */
    getSmsVerifyCode(params) {
      let data = {
        phoneNumber: params.userPhone,
        random:
          Math.random().toString(16).substring(2) +
          Math.random().toString(16).substring(2),
        mode: 0,
        reqType: 3,
        clientType
      };
      if (params.imageCode) {
        // 这里的字段要特别注意，区别正常验证码的，大坑！！！
        data.puzzleVerfycode = Utils.encryptByAesCBC(params.imageCode);
      }
      return new Promise((resolve) => {
        //myRequest('getSmsVerifyCode', params, (result) => resolve(result))
        Utils.sendRequest("/user/sms/getSmsCode", data, { type: "json" }).then(async (result) => {
          let obj = Utils.changeResultData(result);
          if (obj.code == "S_FAIL") {
            if (obj.errorCode == '200059554') {
              // 这里需要请求滑块验证码
              const slideResult = await this.getVerfyCode(params.userPhone);
              if (!obj.var) {
                obj.var = {};
              }
              obj.var.slidePuzzle = slideResult.slidePuzzle;
            }
          }
          resolve(obj);
        }).catch((err) => {
          let obj = {
            code: "S_FAIL_NETWORK",
            msg: "网络异常，请检查网络后重试",
          };
          resolve(obj);
        });
      });
    },

    // 验证码登录
    login(params) {
      var data = {
        clientType,
        cpid,
        dycpwd: params.verifyCode,
        extInfo: {
          ifOpenAccount: 1,
        },
        loginMode: "0",
        msisdn: params.username,
        pintype: "5",
        verType: "2",
        version: window.appVersion,
      };
      return new Promise(async (resolve, reject) => {
        try {
          const result = await Utils.sendRequest("/user/thirdlogin", data, {
            type: "json",
          });
          if (result.data && result.code == '0000') {
            let str = Utils.decryptByAes(result.data);
            if (typeof str === "string") {
              // 数据库登录统一改造
              let userInfo = JSON.parse(str);
              localStorage.setItem('fromUserInfo',JSON.stringify(userInfo))
              try {
                await apiNative.account.addAccountOrModify(userInfo);
              } catch (error) {

              }
            }
          }
          let obj = Utils.changeResultData(result);
          resolve(obj);
        } catch (error) {
          console.error("验证码登录请求出错:", error);
          reject(error);
        }
      });
    },

    // SIM登录
    simLogin(params) {
      let data = {
        clientType,
        cpid,
        dycpwd: params.verifyCode,
        extInfo: {
          appId: "300012063908",
          appKey: "336CF2D0146B51CEDAB502750E7E8C04",
          ifOpenAccount: 1,
          templateId: "DK20210727112418626b47",
        },
        loginMode: "0",
        msisdn: params.phone,
        pintype: "19",
        // "secinfo": "E645623F1F75C2A21B61B6D0723A2B8AB2A7566D",
        verType: "2",
        // 这个版本号，要获取客户端的当前版本号
        version: window.appVersion,
      };
      return new Promise((resolve, reject) => {
        Utils.sendRequest("/user/thirdlogin", data, { type: "json" }).then(
          (result) => {
            // 此接口请求成功，轮询请求结果，逻辑如此
            let json = null;
            if (result.data) {
              let str = Utils.decryptByAes(result.data);
              if (typeof str == "string") {
                json = JSON.parse(str);
              }
            } else {
              // 如果data为空，则直接返回result，给调用方处理
              json = result;
            }
            // 回调出去, 这里要分两步，在外面去调用
            resolve(json);
          }
        ).catch((e) => {
          reject(e);
        });
      })
    },
    // 轮询接口querySendSingleMsgResult.do 查询SIM认证结果，逻辑如此
    querySIMLoginResult(params, responseResult) {
      let data = {
        phoneNumber: params.phone,
        taskId: params.taskID,
        requestType: 1,
      };
      return new Promise((resolve) => {
        // 倒计时60s，一秒请求一次此接口，查sim登录结果
        // 清除定时器
        this.cleanTimer();
        simResultTimer = setInterval(() => {
          if (countDown <= 0) {
            let obj = {
              code: "S_FAIL",
              summary: "请求超时",
            };
            resolve(obj);
            // 清除定时器
            this.cleanTimer();
            return;
          }
          countDown--;
          // 1s请求一次
          Utils.sendRequest("/user/simCertification/getResult", data, { type: "json" }).then(
            async (result) => {
              let resultCode = result?.data?.resultCode;
              let token = result?.data?.token;
              let simResultCode = result?.data?.simResultCode;
              let simState = result?.data?.simState;
              // 代表认证成功
              if (resultCode === "1") {

                // 数据库登录统一改造
                responseResult.token = token;
                responseResult.authToken = token;
                // 缓存接口登录返回用户信息，保证此次登录成功，可以进入首页
                localStorage.setItem('fromUserInfo',JSON.stringify(responseResult))
                try {
                  // 等待 addAccountOrModify 操作完成
                  await apiNative.account.addAccountOrModify(responseResult);
                } catch (error) {
                  console.error("添加或修改账号信息失败", error);
                }
                // 清除定时器
                this.cleanTimer();
                // 如果认证成功拿到token，返回出去，否则继续请求
                let obj = {
                  code: "S_OK",
                  summary: "认证成功",
                };
                resolve(obj);
              } else if (resultCode === "0") {
                // 认证中，继续请求，暂不处理
              } else {
                let errorCode = "";
                let summary = "";
                if (simResultCode === "3002") {
                  // 不支持SIM卡认证
                  summary = "不支持sim认证";
                  errorCode = "3002";
                } else if (simResultCode === "1") {
                  // SIM卡拒绝授权
                  summary = "SIM卡拒绝授权";
                  errorCode = "-10060";
                } else if (simState === "2") {
                  // SIM卡认证超时
                  summary = "SIM卡认证超时";
                  errorCode = "-10061";
                } else {
                  // 请求SIM认证失败，流程结束
                  summary = "SIM认证失败";
                  errorCode = "200059529";
                }
                result.code = "S_FAIL";
                result.errorCode = errorCode;
                result.summary = summary;
                resolve(result);
                this.cleanTimer();
              }
            }
          );
        }, 1000);
      });
    },

    // 清除定时器
    cleanTimer() {
      if (simResultTimer) {
        clearInterval(simResultTimer);
        simResultTimer = null;
        countDown = 60;
      }
    },

    // 账号密码登录
    async accountPasswordLogin(params) {
      let password = Utils.encryptByHash(params.password);
      let data = {
        clienttype: clientType,
        cpid,
        dycpwd: password,
        extInfo: {
          ifOpenAccount: 0,
        },
        loginMode: "0",
        msisdn: params.username,
        pintype: "9",
        secinfo: password,
        verType: "2",
        version: window.appVersion,
      };
      if (params.imageCode) {
        // 这里的字段要特别注意，区别正常验证码的，大坑！！！
        data.verfycode = Utils.encryptByAesCBC(params.imageCode);
      }
      if (params.smsCheckRandom) {
        // 有值，代表是安全登录，修改secinfo参数
        data.secinfo = Utils.safeVerifyCodeEncrypt(
          params.smsCheckRandom + params.username + params.verifyCode
        );
        data.dycpwd = "";
      }
      return new Promise(async (resolve, reject) => {
        try {
          const result = await Utils.sendRequest("/user/thirdlogin", data, { type: "json" });
          if (result.data) {
            let str = Utils.decryptByAes(result.data);
            if (typeof str === "string") {
              // 数据库登录统一改造
              let userInfo = JSON.parse(str);
              // 安全登录接口需要传入，所以在这里缓存下
              protectRecordId = userInfo?.extInfo?.protectRecordId;
              localStorage.setItem('fromUserInfo',JSON.stringify(userInfo))
              try {
                await apiNative.account.addAccountOrModify(userInfo);
              } catch (error) {

              }
            }
          }
          let obj = Utils.changeResultData(result);
          // 错误才需要处理var字段
          if (obj.code === "S_FAIL") {
            let failedResult = {
              protectType: LOGIN_PROTECT_TYPE.NO_LOGIN_PROTECT,
              provCode: 0,
              deviceID: "",
              userPhone: params.username,
              protectRecordID: "",
            };
            if (obj?.var) {
              obj.var.protectType = LOGIN_PROTECT_TYPE.NO_LOGIN_PROTECT;
              obj.var.provCode = 0;
              obj.var.deviceID = "";
              obj.var.userPhone = params.username;
              obj.var.protectRecordID = "";
            } else {
              obj.var = failedResult;
            }
            let protectType = LOGIN_PROTECT_TYPE.NO_LOGIN_PROTECT;
            // 根据 errorCode 设置 protectType
            if (obj.errorCode === "200059526") {
              // CLOUD_LOGIN_CODE_PROTECT_USER
              protectType = LOGIN_PROTECT_TYPE.PWD_LOGIN_PROTECT;
            } else if (obj.errorCode === "200059537") {
              // CLOUD_LOGIN_CODE_REGION_PROTECT
              protectType = LOGIN_PROTECT_TYPE.REGION_LOGIN_PROTECT;
            } else if (obj.errorCode === "200059504") {
              // 暂不知道为啥c端要做一个错误码的转换，来不及研究，只能这样兼容
              obj.errorCode = "200059554";
              // 这里需要请求滑块验证码
              const slideResult = await this.getVerfyCode(params.username);
              obj.var.slidePuzzle = slideResult.slidePuzzle;
            }
            obj.var.protectType = protectType;
          }
          resolve(obj);
        } catch (error) {
          console.error('账号密码登录请求出错:', error);
          reject(error);
        }
      })
    },

    // 获取滑块接口verfycode
    getVerfyCode(account) {
      // 图形验证码类型，目前仅支持2: 滑块验证码
      let data = {
        account: account,
        type: 2,
      };
      return new Promise((resolve) => {
        Utils.sendRequest("/user/verfycode", data, { type: "json" }).then((result) => {
          let obj = Utils.changeResultData(result);
          if (obj.code == 'S_OK') {
            if (obj?.data) {
              // 这里用其他字段接，是为了兼容项目的业务代码，避免改动很大
              obj.slidePuzzle = {
                backGroundImg: obj.data?.picture,
                puzzleImg: obj.data?.puzzle,
                puzzleWidth: obj?.data?.puzzleWidth,
                picWidth: obj?.data?.picWidth,
              }
              resolve(obj);
            }
          }
        });
      });
    },

    // 获取安全登录验证码
    getSafeLoginVerifyCode(params) {
      let smsCheckRandom = Utils.getGUID32();
      let data = {
        phoneNumber: params.userPhone,
        random: smsCheckRandom,
        mode: 0,
        reqType: 9,
        clientType,
      };
      return new Promise((resolve, reject) => {
        Utils.sendRequest("/user/sms/getSmsCode", data, { type: "json" }).then((result) => {
          let obj = Utils.changeResultData(result);
          obj.var = {
            smsCheckRandom: smsCheckRandom,
          };
          resolve(obj);
        });
      });
    },

    // 安全验证码验证
    verifyDyncPasswd(params) {
      let data = {
        phoneNumber: params.username,
        secinfo: Utils.safeVerifyCodeEncrypt(
          params.smsCheckRandom + params.username + params.verifyCode
        ),
        recordID: protectRecordId,
        reqType: 9,
      };
      return new Promise((resolve, reject) => {
        Utils.sendRequest("/user/sms/checkSmsCode", data, { type: "json" }).then(
          (result) => {
            let token = result?.data?.token;
            // 有token，则登录成功
            if (token) {
              /*
                const _userData = JSON.parse(localStorage.getItem("UserInfo"));
                _userData["token"] = result.token;
                _userData["authToken"] = result.token;
                localStorage.setItem('UserInfo',JSON.stringify(_userData));
                */

              // 数据库登录统一改造
              const userData = JSON.parse(localStorage.getItem("UserInfo"));
              userData.token = token;
              userData.authToken = token;
              // let modifyUserInfo = Utils.handleLoginInfoForNode(userData);
              // 安全验证登录成功，修改数据库登录状态accountState=0，表示登录
              // 修改账号信息，这里需要传递id
              // 首先需要通过userDomainId拿到数据库对应的数据
              apiNative.account.getAccountInfo(userData.userDomainId, res => {
                if (res) {
                  // 调用updateAccountField方法
                  userData.userid = res.id;
                  let fieldItem = {
                    accountState: 0,
                    token: token
                  }
                  window.nativeAbility.logger.info('开始安全登录updateAccountField修改数据库'+JSON.stringify(userData))
                  window.nativeAbility.logger.info('updateAccountField修改数据库所需字段' + String(userData.userid) +JSON.stringify(fieldItem))
                  apiNative.account.updateAccountField(userData.userid, fieldItem).then(res => {
                      // 缓存登录用户信息
                      let userInfoStr = JSON.stringify(userData);
                      let fromUserInfo =  localStorage.getItem('fromUserInfo')
                      fromUserInfo = JSON.parse(fromUserInfo);
                      fromUserInfo.token = userData.token;
                      fromUserInfo.authToken = userData.token;
                      fromUserInfo.userid = userData.userid;
                      localStorage.setItem('fromUserInfo', JSON.stringify(fromUserInfo))
                      localStorage.setItem('UserInfo', userInfoStr);
                      window.nativeAbility.logger.info('账号存在，修改账号返回结果'+JSON.stringify(res))
                  });
                }
              })
              // apiNative.account.modifyAccount(modifyUserInfo);
            }
            let obj = Utils.changeResultData(result);
            resolve(obj);
          }
        );
      });
    },

    // 二维码登录
    async QRLogin(params) {
      let data = {
        clientType,
        cpid,
        dycpwd: params.sessionId,
        extInfo: {
          ifOpenAccount: 0,
        },
        loginMode: "0",
        msisdn: "",
        pintype: "21",
        verType: "2",
        version: window.appVersion,
      };
      return new Promise(async (resolve, reject) => {
        try {
          const result = await Utils.sendRequest("/user/thirdlogin", data, {
            type: "json",
          });
          if (result.data) {
            let str = Utils.decryptByAes(result.data);
            if (typeof str == "string" && result.code == '0000') {
              // 停止定时器，避免继续轮询
              params.timer && params.timer();
              // 数据库登录统一改造
              let userInfo = JSON.parse(str);
              // 存一下user_unique_id, 埋点用
              localStorage.setItem("user_unique_id", userInfo.account);
              localStorage.setItem('fromUserInfo',JSON.stringify(userInfo))
              try {
                await apiNative.account.addAccountOrModify(userInfo);
              } catch (error) {

              }
            }
          }
          let obj = Utils.changeResultData(result);
          resolve(obj);
        } catch (error) {
          // 捕获 utils.sendRequest 中的错误信息
          console.error("二维码登录请求出错:", error);
          reject(error);
        }
      });
    },
    // 获取客户端设备信息
    async getClientDeviceInfo() {
      if (window?.nativeAbility?.common) {
        try {
          const res = await window.nativeAbility.common.getDeviceInfo();
          console.log('获取客户端设备信息', res);
          if (res.success) {
            // 显式转换为字符串存储
            localStorage.setItem("windowMachine", res.machine + '-ENDIN');
            localStorage.setItem("windowDeviceInfo", res.deviceinfo);
          }
        } catch (error) {
          console.error('获取客户端设备信息失败:', error);
        } finally {
          let windowMachine = localStorage.getItem('windowMachine');
          let windowDeviceInfo = localStorage.getItem('windowDeviceInfo');
          if (!windowMachine) {
            windowMachine = Utils.getGUID32() + '-ENDIN';
            localStorage.setItem('windowMachine', windowMachine);
          }
          if (!windowDeviceInfo) {
            windowDeviceInfo = `||11|${appVersion}|PC|QkYtMjAyMDAzMTAxNjQ3|${windowMachine}|| Windows 10 (10.0)|1920X1040|Q2hpbmVzZSAoU2ltcGxpZmllZCk=|||`;
            localStorage.setItem('windowDeviceInfo', windowDeviceInfo);
          }
        }
      }
    },
    // 生成登录二维码
    async createLoginQRcode() {
      // 如果c端返回了，就用c端的windowDeviceId
      let deviceId = localStorage.getItem("windowMachine");
      if (!deviceId) {
          await this.getClientDeviceInfo();
          deviceId = localStorage.getItem("windowMachine");
      }
      store.commit("SET_WINDOWMACHINE", deviceId);
      let sessionId = Utils.getGUID32();
      // 创建二维码的时候，生成随机数，保存下来，当做唯一标识deviceId，扫码登录轮询需要用到，因为刷新这个设备id也会变，所以放在这里
      // 如果存在就取之前的，保证唯一性，先这样做，后期可根据设备的信息生成32位的deviceId
      let QRcodeUrl = "https://yun.139.com/w/#/qrcLogin";
      let QRCodePath =
        QRcodeUrl +
        "?dID=" +
        deviceId +
        "&sID=" +
        sessionId +
        "&cType=11&ver=" +
        window.appVersion;
      // 需要生成二维码图片
      return new Promise(async (resolve, reject) => {
        let result = {
          code: "S_OK",
          var: {
            // QRCodeID也是sessionId
            QRCodeID: sessionId,
            QRCodePath: QRCodePath,
          },
        };
        await Utils.generateQRCode(QRCodePath).then((path) => {
          result.var.QRCodePath = path;
        });
        resolve(result);
      });
    },

    // 生成授权登录二维码
    createAuthLoginCode(data) {
      // 如果c端返回了，就用c端的windowDeviceId
      let deviceId = window.windowDeviceId;
      if (!deviceId) {
        deviceId = Utils.getGUID32();
        localStorage.setItem("deviceId", deviceId);
      }
      let authQRcodeUrl = "https://caiyun.feixin.10086.cn:7071/portal/app/launch.html";
      let QRcodeInfo = {
        account: data.account,
        deviceID: deviceId,
        authRecordID: data?.extInfo?.logInAuthInfoRecordId
      }
      let base64EncodeStr = btoa(JSON.stringify(QRcodeInfo));
      let QRCodePath = authQRcodeUrl + "?userInfo=" + base64EncodeStr + "&url=mcloud://untrustedDeviceAuthorize";
      return new Promise(async (resolve, reject) => {
        let result = {
          code: "S_OK",
          var: {
            QRCodePath: QRCodePath,
          },
        };
        await Utils.generateQRCode(QRCodePath).then((path) => {
          result.var.QRCodePath = path;
        });
        resolve(result);
      });
    },

    // 统一处理授权二维码逻辑
    async getAuthLoginCodeResult(result) {
      if (!result) {
        return;
      }
      return await this.authPreLogin(result);
    },


    // 授权弹出框以后，开始预登陆, 成功以后在开始轮询授权二维码接口
    async authPreLogin(result) {
      localStorage.setItem('fromUserInfo',JSON.stringify(result))
      try {
        await apiNative.account.addAccountOrModify(result);
      } catch (error) {

      }
      return await this.rooloopAuthLoginCodeResult(result);
    },

    // 获取授权二维码轮询结果
    rooloopAuthLoginCodeResult(json) {
      // 先登录一次，这里仅缺少token，token通过轮询授权二维码接口获取，拿到token在重新赋值本地和数据库
      let data = {
          queryType: 1,
          authRecordID: json?.extInfo?.logInAuthInfoRecordId || '',
          deviceID: json?.extInfo?.primDevceID,
          phoneNumber: json?.account
      }
      return new Promise((resolve) => {
        // 清除定时器
        this.cleanAuthQRCodeTimer();
        authQRCodeTimer = setInterval(() => {
          Utils.sendRequest("/user/device/getUserAuthDevInfo", data, {type: "json" }).then(result => {
            let obj = Utils.changeResultData(result);
            console.log('获取授权二维码轮询结果', obj);
            // 轮询结果
            if (obj.code === "S_OK") {
              let userAuthInfoList = obj?.data?.userAuthDevInfoList || [];
              if (userAuthInfoList) {
                userAuthInfoList.forEach(async item => {
                  let deviceId = item?.deviceId;
                  let authResult = item?.authResult;
                  let token = item?.token;
                  if (deviceId === json?.extInfo?.primDevceID) {
                    //  授权成功，拿到token，开始登录
                    if (authResult == 1 && token) {
                      // 表示授权成功, 且拿到了token
                      obj.authSuccess = true;
                      this.cleanAuthQRCodeTimer();
                      // 登录
                      // 数据库登录统一改造
                      await this.updateAuthUserInfo(token);
                      resolve(obj);
                      return;
                    } else if (authResult == 2) {
                      obj.summary = "拒绝授权";
                      obj.code = "S_FAIL";
                      this.cleanAuthQRCodeTimer();
                      resolve(obj);
                    }
                  }
                })

              }
            } else {
              obj.code = "S_FAIL";
              obj.summary = "请求失败，请稍后再试"
              this.cleanAuthQRCodeTimer();
              resolve(obj);
            }
            // 还未授权，等待授权
            obj.summary = "还未授权，等待授权"
          }).catch((err) => {
            console.error('授权二维码轮询请求失败:', err);
            let obj = {
              code: "S_FAIL_NETWORK",
              msg: "网络异常，请检查网络后重试",
            };
            this.cleanAuthQRCodeTimer();
            resolve(obj);
          });
        }, 1000);

      });
    },

    // 清除定时器
    cleanAuthQRCodeTimer() {
      if (authQRCodeTimer) {
        clearInterval(authQRCodeTimer);
        authQRCodeTimer = null;
      }
    },

    async updateAuthUserInfo(token) {
      try {
        // 数据库登录统一改造
        const userData = JSON.parse(localStorage.getItem("UserInfo"));
        userData.token = token;
        userData.authToken = token;

        console.log('数据库登录统一改造', userData);
        // 通过 userDomainId 拿到数据库对应的数据
        return new Promise((resolve, reject) => {
          apiNative.account.getAccountInfo(userData.userDomainId, res => {
            if (res) {
              // 调用 updateAccountField 方法
              userData.userid = res.id;
              let fieldItem = {
                accountState: 0,
                token: token
              };
              window.nativeAbility.logger.info('开始授权登录 updateAccountField 修改数据库'+JSON.stringify(userData));

              apiNative.account.updateAccountField(userData.userid, fieldItem).then(res => {
                // 缓存登录用户信息
                let userInfoStr = JSON.stringify(userData);
                let fromUserInfo = localStorage.getItem('fromUserInfo');
                fromUserInfo = JSON.parse(fromUserInfo);
                fromUserInfo.token = userData.token;
                fromUserInfo.authToken = userData.authToken;
                fromUserInfo.userid = userData.userid;
                localStorage.setItem('fromUserInfo', JSON.stringify(fromUserInfo));
                localStorage.setItem('UserInfo', userInfoStr);
                window.nativeAbility.logger.info('账号存在，修改账号成功'+JSON.stringify(res));
                resolve(res);
              }).catch(err => {
                reject(err);
              });
            } else {
              resolve(null);
            }
          });
        });
      } catch (error) {
        console.error('更新授权用户信息异常:', error);
        throw error;
      }
    },

    // 获取统一通行证临时票据
    getCloudTicket(toSourceId) {
      var requestUrl = "/user/querySpecTokenV2";
      var requestParams = {
        toSourceId: toSourceId || "001003",
      };
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },

    // 获取RSA公钥
    getPublicRSAKey() {
      var requestUrl = "/caiyun/openapi/authentication/key/getRsaPublicKey";
      var requestParams = {
        clientCode: "0010302", // PC Windows填10301 PC Mac填10302 UOS填10303
        type: 1,
      };
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },

    /**
     * 打开浏览器
     * @param {重定向地址} redirectUrl
     * @param {是否打开文档} isOpenDocument
     */
    async openWebBrowser(redirectUrl, isOpenDocument = false) {
      try {
        let requestUrl = "/user/querySpecTokenV2";
        let requestParams = { toSourceId: "001005" };

        const res1 = await Utils.sendRequest(requestUrl, requestParams, { type: "json" });
        console.log("统一通行证临时票据", res1);

        if (res1.code !== "0000") {
          throw new Error('临时票据获取失败');
        }

        var ticket = res1?.data?.token;
        var jsonData = {
          dycPwd: ticket,
          loginStyle: "passTmpTicker",
        };
        var plaintext = JSON.stringify(jsonData);
        var RSAKey = Tools.randomString(16);
        var ciphertext = Utils.aes_ecb_encrypt(plaintext, RSAKey, "");

        requestUrl = "/caiyun/openapi/authentication/key/getRsaPublicKey";
        requestParams = { clientCode: "10301", type: 1 };

        const res2 = await Utils.sendRequest(requestUrl, requestParams, { type: "json" });

        if (res2.statusCode !== 200) {
          throw new Error('公钥获取失败');
        }

        var PublicRSAKey = res2.data;
        var publicKey = Utils.makeNetKey(PublicRSAKey, 1);
        var encryptSkey = Utils.publicEncryptRSA(RSAKey, publicKey, 1024);

        var url = `https://yun.139.com/caiyun/openapi/authentication/singleLogin?autoLogin=false&clientCode=10301&channel=0010112&encryptMsg=${encodeURIComponent(ciphertext)}&sKey=${encodeURIComponent(encryptSkey)}&redirectUrl=${encodeURIComponent(redirectUrl)}`;

        if (isOpenDocument) {
          url = `https://yun.139.com/orchestration/auth-rebuild/permission/v1.0/singleLogin?autoLogin=false&encryptMsg=${encodeURIComponent(ciphertext)}&sKey=${encodeURIComponent(encryptSkey)}&redirectUrl=${encodeURIComponent(redirectUrl)}`;
        }

        window.open(url, "_blank");
        return true;

      } catch (error) {
        console.error('登录流程错误:', error);
        return false;
      }
    },
    // 打开智能客服
    openAIWebChat() {
      return new Promise((resolve) => {
        let requestUrl = "/user/querySpecTokenV2";
        let requestParams = {
          toSourceId: "001248",
        };
        Utils.sendRequest(requestUrl, requestParams, { type: "json" })
          .then((res) => {
            if (res.code === "0000") {
              var token = res?.data?.token;
              var plaintext = "hcy";
              var AESKey = "FCMCC10086000000";
              var ciphertext = Utils.aes_ecb_encrypt(plaintext, AESKey, "");
              var encryptProduct = Utils.base64ToHexUpper(ciphertext);
              var url =
                "https://www.cmccaikf.com/cs-wap/sso/ssoLogin?product=" +
                encryptProduct +
                "&token=" +
                token +
                "&targetsourceid=001005&channel=pc";
              window.open(url, "_blank");
              resolve(ture);
            } else {
              resolve(false);
            }
          })
          .catch(() => {
            resolve(false);
          });
      });
    },
    // PDF转换Token查询（RHR135权益）：benefitValue - benefitUsed = 剩余Token
    getTokenBalance(params) {
      var requestUrl = "/isbo/openApi/queryAvailableBenefitV2";
      return Utils.sendRequest(requestUrl, params, { type: "json" }).then(res => {
        if (res?.result?.resultCode === '0' || res?.code === '0000') {
          const benefitList = res.userAvailableBenefitList || res.data?.userAvailableBenefitList || res.result?.userAvailableBenefitList || [];
          const tokenBenefit = benefitList.find(b => b.benefitNo === 'RHR135');

          let tokenBalance = 0;
          if (tokenBenefit) {
            const total = parseInt(tokenBenefit.benefitValue || 0, 10);
            const used = parseInt(tokenBenefit.benefitUsed || 0, 10);
            tokenBalance = Math.max(0, total - used);
            console.log('PDF Token余额计算完成:', {
              benefitNo: 'RHR135',
              total,
              used,
              remaining: tokenBalance,
              benefitStatus: tokenBenefit.benefitStatus
            });
          }

          return {
            code: '0000',
            data: { token: tokenBalance, ...tokenBenefit }
          };
        }
        console.warn('Token查询响应异常:', res);
        return { code: '9999', data: { token: 0 } };
      }).catch(err => {
        console.error('getTokenBalance失败:', err);
        return { code: '9999', data: { token: 0 } };
      });
    },
    // 获取白银会员权限
    silverAuthQuery() {
      var requestUrl = "/isbo2/openApi/queryMbBenefitInfoV2";
      let requestParams = {
         extInfo:{
            ignoreIsVisible:1
         }
      }
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 获取非会员权益
    unAuthQuery() {
      var requestUrl = "/isbo2/openApi/queryAvailableBenefitV2";
      let requestParams = {
        account: {
          accountName:window.userDomainId || window.UserInfo.userDomainId,
          accountType:"1",
        },
      }
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 获取会员权限
    memberAuthQuery() {
      var requestUrl = "/isbo2/openApi/queryUserBenefitsV2";
      let requestParams = {
            account: {
              accountName:window.userDomainId || window.UserInfo.userDomainId
            },
            isNeedBenefit:1,
        }
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    getEditVision(params) {
      var requestUrl = "/yun-note/configCenter/getEditorConfig";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
  },
  // 保险箱模块
  safeBox: {
    // 获取保险箱登录状态
    getSafeBoxLoginStatus() {
      let requestUrl = "/hcy/safebox/status";
      return Utils.sendRequest(requestUrl, {}, { type: "json" });
    },

    // 查询用户设置的保险箱安全问题和安全邮箱qryUserSecQuesAndSecMail
    queryUserSecQuesAndSecMail() {
      let requestUrl = "/hcy/safebox/getUserSecQuesAndSecMai";
      return Utils.sendRequest(requestUrl, {}, { type: "json" });
    },

    // 获取公共publicKey
    getPublicKey() {
      let requestUrl = "/hcy/safebox/getPublicKey";
      return Utils.sendRequest(requestUrl, {}, { type: "json" });
    },

    // 登录保险箱
    safeBoxAppLogin(params) {
      let requestUrl = "/hcy/safebox/login";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },

    // 刷新保险箱有效期
    safeboxRefreshSession() {
      let requestUrl = "/hcy/safebox/renewSession";
      let options = {
        type: "json",
      };

      let safeSessionInfo = JSON.parse(
        localStorage.getItem("safeSessionInfo")
      );
      if (safeSessionInfo) {
        options.appendHeads = {
          "x-yun-sbox-session-id": safeSessionInfo.sessionID,
        };
      }
      return Utils.sendRequest(requestUrl, {},options);
    },
    // 获取保险箱问题
    getSafeBoxQuestions() {
      let requestUrl = "/hcy/safebox/listSysSecQuestions";
      return Utils.sendRequest(requestUrl, {}, { type: "json" });
    },

    // 获取保险箱手机验证码
    getSafePhoneCode(data) {
      let requestUrl = "/hcy/safebox/applyPhoneIdenCode";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 查询用户设置的保险箱安全问题和安全邮箱
    getUserSecQuesAndSecMai() {
      let requestUrl = "/hcy/safebox/getUserSecQuesAndSecMai";
      return Utils.sendRequest(requestUrl, {}, { type: "json" });
    },

    // 保险箱安全邮箱验证码下发getSecMailIdenCode
    getSecMailIdenCode(data) {
      let requestUrl = "/hcy/safebox/applySecMailIdenCode";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 验证保险箱邮箱验证码验证
    verSecMailIdenCode(data) {
      let requestUrl = "/hcy/safebox/verifySecMailIdenCode";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 校验保险箱安全问题版本2
    safeBoxVerSecQuestionV2(data) {
      let requestUrl = "/hcy/safebox/verifySecQuestion";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 验证手机验证码
    verifyPhoneIdenCode(data) {
      let requestUrl = "/hcy/safebox/verifyPhoneIdenCode";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 首次申请设置保险箱安全邮箱验证码(云空间)
    firstApplySecMail(data) {
      let requestUrl = "/hcy/safebox/firstApplySecMail";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 首次设置保险箱邮箱和安全问题（初始化保险箱）
    firstSetSbox(data) {
      let requestUrl = "/hcy/safebox/firstSetSbox";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 设置保险箱密码 版本4， 解决保险箱安全问题
    safeBoxSetAppPwdV4(data) {
      let requestUrl = "/hcy/safebox/setPin";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 查询sim签名的结果
    querySimLoginResult(data) {
      let requestUrl = "/hcy/safebox/querySimLoginResult";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },

    // 保险箱的sim登录
    safeboxSimLogin() {
      let requestUrl = "/hcy/safebox/simLogin";
      return Utils.sendRequest(requestUrl, {}, { type: "json" });
    },
  },

  user: {
    getUserInfo(params) {
      let requestUrl = "/richlifeApp/devapp/IUser";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    // 获取用户头像和昵称
    getUserAvatar(params) {
      let requestUrl = "/hcy/family/adapter/andAlbum/openApi/getUserInfo";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    // 获取用户会员信息
    getUserMemberInfo(params) {
      let requestUrl = "/m4c/openapi/userIdentity";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    // 获取用户割接状态和底座信息
    getUserStatus(params) {
      let requestUrl = "/user/status/query";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    // 获取用户接口域
    getUserRoutePolicy(params) {
      let requestUrl = "/user/route/qryRoutePolicy";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    // 获取会员中心二维码
    getMemberCenterQrCode() {
      let url =
        "https://vip.caiyun.feixin.10086.cn/vip/?sourceid=10021&account=" +
        window.userAccount; // ToDo: 地址拼接规则写死，后续可以优化
      return Utils.generateQRCode(url);
    },
    // 查询用户PDF转格式剩余体验次数
    getSurplusNum(params) {
      let requestUrl = "/hcy/file/online/wps/surplusNum";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
  },
  personal: {
    orderTypeObj: {
      // created_at: 根据创建时间排序
      0: "updated_at", // updated_at：根据更新时间排序
      1: "name", // name：根据文件名排序
      2: "", //     : 类型
      3: "size", // size：根据文件大小排序
      4: "", //     : 后缀名
      5: "starred_at", // 收藏时间
    },
    orderDescObj: {
      0: "ASC", // 正排序
      1: "DESC", // 倒排序
    },
    getList(item) {
      var userAccount = window.userAccount;
      var requestUrl, requestParams, type;
      if (isCutOver()) {
        // 割接接口
        type = "json";
        requestUrl = "/hcy/file/list";
        requestParams = {
          // userId: window.userId,
          // 是根目录使用"/"
          parentFileId: item.directoryId.includes("00019700101000000001")
            ? "/"
            : item.directoryId,
          pageInfo: {
            pageSize: item.pageSize,
            pageCursor: item.nextPageCursor || "",
          },
          orderBy: this.orderTypeObj[item.contentSortType],
          orderDirection: this.orderDescObj[item.sortDirection],
          type: item.type,
          imageThumbnailStyleList: ["Small", "Middle", "Big", "Large"],
        };
        requestParams = JSON.stringify(requestParams);
      } else {
        if (item.catalogSortType > 4) {
          item.catalogSortType = 0;
          item.contentSortType = 0;
        }
        type = "xml";
        requestUrl = "/richlifeApp/devapp/ICatalog";

        requestParams = {
          getDisk: {
            MSISDN: userAccount,
            catalogID: item.directoryId,
            filterType: item.filterType,
            catalogSortType: item.catalogSortType,
            contentType: 0,
            contentSortType: 1,
            sortDirection: item.sortDirection,
            startNumber: 1 + item.pageSize * (item.toPage - 1),
            endNumber: item.pageSize + item.pageSize * (item.toPage - 1),
            catalogType: -1,
            auditFilterFlag: 2,
          },
        };
      }
      let options = {
        type: type,
      };
      if (item.module === "safeBox") {
        let safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
      }
      return Utils.sendRequest(requestUrl, requestParams, options);
    },
    //获取挂载盘文件夹、文件列表
    mountDiskGetList(item) {
      var requestUrl, requestParams, type;
      requestUrl = "/mount/file/list";
      requestParams = {
        parentFileId: item.directoryId.includes("00019700101000000001") ? "/" : item.directoryId,
        pageInfo: {
          pageSize: item.pageSize,
          pageCursor: item.nextPageCursor || "",
        },
        orderDirection: this.orderDescObj[item.sortDirection],
        type: item.type,
        imageThumbnailStyleList: ["Small", "Middle", "Big", "Large"],
      };
      if (this.orderTypeObj[item.contentSortType]) {
        requestParams.orderBy = this.orderTypeObj[item.contentSortType];
      } else {
        // 为空时，传默认的'updated_at'值，防止后台报字段不合法错误
        requestParams.orderBy = 'updated_at';
      }
      console.log("mountDiskGetList requestParams:", requestParams);
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    getListCache(item,callback,isChace=false) {
      var userAccount = window.userAccount;
      var requestUrl, requestParams, type;
      if (isCutOver()) {
        // 割接接口
        type = "json";
        requestUrl = "/hcy/file/list";
        requestParams = {
          // userId: window.userId,
          // 是根目录使用"/"
          parentFileId: item.directoryId.includes("00019700101000000001")
            ? "/"
            : item.directoryId,
          pageInfo: {
            pageSize: item.pageSize,
            pageCursor: item.nextPageCursor || "",
          },
          orderBy: this.orderTypeObj[item.contentSortType],
          orderDirection: this.orderDescObj[item.sortDirection],
          type: item.type,
          imageThumbnailStyleList: ["Small", "Middle", "Big", "Large"],
        };
        requestParams = JSON.stringify(requestParams);
      } else {
        if (item.catalogSortType > 4) {
          item.catalogSortType = 0;
          item.contentSortType = 0;
        }
        type = "xml";
        requestUrl = "/richlifeApp/devapp/ICatalog";

        requestParams = {
          getDisk: {
            MSISDN: userAccount,
            catalogID: item.directoryId,
            filterType: item.filterType,
            catalogSortType: item.catalogSortType,
            contentType: 0,
            contentSortType: 1,
            sortDirection: item.sortDirection,
            startNumber: 1 + item.pageSize * (item.toPage - 1),
            endNumber: item.pageSize + item.pageSize * (item.toPage - 1),
            catalogType: -1,
            auditFilterFlag: 2,
          },
        };
      }
      let options = {
        type: type,
      };
      if (item.module === "safeBox") {
        let safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
      }
      const reqBack= Utils.sendRequest(requestUrl, requestParams, options)
      getCacheDataFn(requestUrl,requestParams,callback,reqBack,isChace);
    },
    getCollection(item) {
      var userAccount = window.userAccount;
      var requestUrl, requestParams, type;
      if (isCutOver()) {
        // 割接接口
        type = "json";
        requestUrl = "/hcy/bookmark/listFiles";
        requestParams = {
          pageInfo: {
            pageSize: item.pageSize,
            pageCursor: item.nextPageCursor || "",
          },
          orderBy: this.orderTypeObj[item.contentSortType],
          orderDirection: this.orderDescObj[item.sortDirection],
          imageThumbnailStyleList: ["Small", "Middle", "Big", "Large"],
        };
        requestParams = JSON.stringify(requestParams);
      } else {
        if (item.catalogSortType > 4) {
          item.catalogSortType = 0;
          item.contentSortType = 0;
        }
        type = "xml";
        requestUrl = "/richlifeApp/devapp/ICatalog";

        requestParams = {
          getDisk: {
            MSISDN: userAccount,
            catalogID: item.directoryId,
            filterType: item.filterType,
            catalogSortType: item.catalogSortType,
            contentType: 0,
            contentSortType: 1,
            sortDirection: item.sortDirection,
            startNumber: 1 + item.pageSize * (item.toPage - 1),
            endNumber: item.pageSize + item.pageSize * (item.toPage - 1),
            catalogType: -1,
            auditFilterFlag: 2,
          },
        };
      }
      return Utils.sendRequest(requestUrl, requestParams, { type });
    },
    getCollectionCache(item,callback) {
      var userAccount = window.userAccount;
      var requestUrl, requestParams, type;
      if (isCutOver()) {
        // 割接接口
        type = "json";
        requestUrl = "/hcy/bookmark/listFiles";
        requestParams = {
          pageInfo: {
            pageSize: item.pageSize,
            pageCursor: item.nextPageCursor || "",
          },
          orderBy: this.orderTypeObj[item.contentSortType],
          orderDirection: this.orderDescObj[item.sortDirection],
          imageThumbnailStyleList: ["Small", "Middle", "Big", "Large"],
        };
        requestParams = JSON.stringify(requestParams);
      } else {
        if (item.catalogSortType > 4) {
          item.catalogSortType = 0;
          item.contentSortType = 0;
        }
        type = "xml";
        requestUrl = "/richlifeApp/devapp/ICatalog";

        requestParams = {
          getDisk: {
            MSISDN: userAccount,
            catalogID: item.directoryId,
            filterType: item.filterType,
            catalogSortType: item.catalogSortType,
            contentType: 0,
            contentSortType: 1,
            sortDirection: item.sortDirection,
            startNumber: 1 + item.pageSize * (item.toPage - 1),
            endNumber: item.pageSize + item.pageSize * (item.toPage - 1),
            catalogType: -1,
            auditFilterFlag: 2,
          },
        };
      }
      const reqBack= Utils.sendRequest(requestUrl, requestParams, { type })
      getCacheDataFn(requestUrl,requestParams,callback,reqBack,!JSON.parse(requestParams).pageCursor);
    },
    // 收藏文件
    addCollection(params) {
      var requestUrl = "/hcy/bookmark/addFiles";
      var requestParams = {
        fileIds: params.list.map((item) => {
          return item.id;
        }), // [id,id,id]
      };
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 取消收藏
    delCollection(params) {
      var requestUrl = "/hcy/bookmark/removeFiles";
      var requestParams = {
        fileIds: params.list.map((item) => {
          return item.id;
        }), // [id,id,id]
      };
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 查询是否开启智能相册
    getAlbumSwitch() {
      var requestUrl = "/album/timeline/intelligent/user/ai/get";
      var requestParams = {
        userId: window.UserInfo.userDomainId,
      };
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 获取智能相册-人物列表
    getPersonList(data) {
      var requestUrl = "/album/classify/person/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取智能相册-地点列表
    getLocationList(data) {
      var requestUrl = "/album/classify/addr/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取智能相册-事物列表
    getThingList(data) {
      var requestUrl = "/album/classify/thing/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取智能相册-列表数据
    getAlbumFileList(data, type) {
      let requestUrl;
      if (type === "figure") {
        requestUrl = "/album/classify/person/file/list";
      } else if (type === "address") {
        requestUrl = "/album/classify/addr/file/list";
      } else if (type === "things") {
        requestUrl = "/album/classify/thing/file/list";
      }
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 智能相册-人物相册重命名
    renamePersonAlbum(data) {
      var requestUrl = "/album/classify/person/update";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 查询相册收录范围
    getAlbumScope(data) {
      var requestUrl = "/album/timeline/user/range/get";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 设置相册收录范围
    setAlbumScope(data) {
      var requestUrl = "/album/timeline/user/range/set";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取相册和视频列表
    getAlbum(item, type) {
      type = Number(type);
      var userAccount = window.userAccount;
      let body, requestUrl, requestName;
      var orderBy = "";
      orderBy = item.orderBy === 0 ? 2 : 1;
      // 云空间，pds和云空间使用一样的接口了
      if (isCutOver() || window.belongsPlatform == 1) {
        requestName = "getIngestedInfo";
        requestUrl = "/album/timeline/get";
        body = {
          userId: window.UserInfo.userDomainId,
          pageInfo: {
            pageCursor: item.nextPageCursor || "",
            pageSize: item.pageSize,
          },
          orderBy: orderBy,
          // 升序2，降序1
          orderDirection: item.sortDirection === 0 ? 2 : 1,
          thumbnailStyleList: ["Small", "Big", "Middle", "Large"],
        };
        //body = JSON.stringify(body);
        return Utils.sendRequest(requestUrl, body, { type: "json" });
      }
    },
    // 获取相册和视频列表
    getAlbumCache(item, callback) {
      var userAccount = window.userAccount;
      let body, requestUrl, requestName;
      var orderBy = "";
      orderBy = item.orderBy === 0 ? 2 : 1;
      // 云空间，pds和云空间使用一样的接口了
      if (isCutOver() || window.belongsPlatform == 1) {
        requestName = "getIngestedInfo";
        requestUrl = "/album/timeline/get";
        body = {
          userId: window.UserInfo.userDomainId,
          pageInfo: {
            pageCursor: item.nextPageCursor || "",
            pageSize: item.pageSize,
          },
          orderBy: orderBy,
          // 升序2，降序1
          orderDirection: item.sortDirection === 0 ? 2 : 1,
          thumbnailStyleList: ["Small", "Big", "Middle", "Large"],
        };
        const reqBack= Utils.sendRequest(requestUrl, body, { type: "json" })
        getCacheDataFn(requestUrl,JSON.stringify(body) ,callback,reqBack,!body.pageInfo.pageCursor);
      }
    },
    getVideo(item, type) {
      if (item.orderBy == 5) {
        item.orderBy = 0;
      }
      var orderBy = this.orderTypeObj[item.orderBy];
      // 云空间，pds和云空间使用一样的接口了
      if (isCutOver() || window.belongsPlatform == 1) {
        var requestUrl = "/hcy/file/search";
        var body = {
          categoryList: ["video"],
          pageInfo: {
            pageCursor: item.nextPageCursor || "",
            pageSize: item.pageSize,
          },
          orderBy: orderBy,
          orderDirection: this.orderDescObj[item.sortDirection],
          parentFileId: "/",
          recursive: true,
          thumbnailStyleList: ["Small", "Big", "Middle", "Large"],
        };
        //body = JSON.stringify(body);
        return Utils.sendRequest(requestUrl, body, { type: "json" });
      }
    },
    getVideoCache(item,callback) {
      if (item.orderBy == 5) {
        item.orderBy = 0;
      }
      var orderBy = this.orderTypeObj[item.orderBy];
      // 云空间，pds和云空间使用一样的接口了
      if (isCutOver() || window.belongsPlatform == 1) {
        var requestUrl = "/hcy/file/search";
        var body = {
          categoryList: ["video"],
          pageInfo: {
            pageCursor: item.nextPageCursor || "",
            pageSize: item.pageSize,
          },
          orderBy: orderBy,
          orderDirection: this.orderDescObj[item.sortDirection],
          parentFileId: "/",
          recursive: true,
          thumbnailStyleList: ["Small", "Big", "Middle", "Large"],
        };
        const reqBack= Utils.sendRequest(requestUrl, body, { type: "json" })
        getCacheDataFn(requestUrl,JSON.stringify(body) ,callback,reqBack,!body.pageInfo.pageCursor);
      }
    },
    // 获取在线预览信息
    async getPreviewInfo(item,ismountdisk = false) {
      var requestUrl = "/hcy/videoPreview/getPreviewInfo";
      if (ismountdisk) {
        requestUrl = "/mount/videoPreview/getPreviewInfo"
      }

      var requestParams = {
        userDomainId: item.userDomainId,
        fileId: item.fileId,
        category: item.category,
        expireSec: 14400, //过期时间最大14400s（4h）， 默认900s（15min）
      };
      let options = {
        type: "json",
      };
      if (item.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete item.isSafeBox;
      }
      requestParams = JSON.stringify(requestParams);

      if (item.category == "video") {
        let enable = await apiNative.global.getNetworkOptConfig();
        let isNonLocal = await apiNative.global.isNonLocalNetwork(Tools.getPublicUrl("ip-info") || "");
        console.log('异网开关enable：', enable, '异网检测isNonLocal：', isNonLocal);
        if (enable && isNonLocal) {
          if (!options.appendHeads) {
            options.appendHeads = {};
          }
          options.appendHeads["x-yun-url-type"] = 3;
        }
      }
      return Utils.sendRequest(requestUrl, requestParams, options);
    },
    // 获取文件版本信息
    getFileVerInfo(item) {
      var requestUrl = "/hcy/revision/listRevision";
      var requestParams = {
        fileId: item.id || item.fileId || item.contentID,
        pageInfo: {
          pageSize: 100,
          pageCursor: "",
        },
      };
      let options = {
        type: "json",
      };
      if (item?.businessType === 4 || item?.sessionId?.length > 0) {
        let safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
      }
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, options);
    },
    // 恢复文件历史版本
    restoreFile(item) {
      var requestUrl = "/hcy/revision/restoreRevision";
      let options = {
        type: "json",
      };
      if (item.sessionId) {
        options.appendHeads = {
          "x-yun-sbox-session-id": item.sessionId,
        };
      }
      return Utils.sendRequest(requestUrl, item, options);
    },
    getTypeList(item, type) {
      var orderBy = this.orderTypeObj[item.orderBy || item.contentSortType];
      // 云空间，pds和云空间使用一样的接口了
      if (isCutOver() || window.belongsPlatform == 1) {
        var requestUrl = "/hcy/file/search";
        var body = {
          categoryList: [type],
          pageInfo: {
            pageCursor: item.nextPageCursor || "",
            pageSize: 100, //item.pageSize,
          },
          orderBy: orderBy,
          orderDirection: this.orderDescObj[item.sortDirection],
          parentFileId: "/",
          recursive: true,
          thumbnailStyleList: ["Small", "Big", "Middle", "Large"],
        };
        //body = JSON.stringify(body);
        return Utils.sendRequest(requestUrl, body, { type: "json" });
      }
    },
    getTypeListCache(item, type,callback) {
      var orderBy = this.orderTypeObj[item.orderBy || item.contentSortType];
      // 云空间，pds和云空间使用一样的接口了
      if (isCutOver() || window.belongsPlatform == 1) {
        var requestUrl = "/hcy/file/search";
        var body = {
          categoryList: [type],
          pageInfo: {
            pageCursor: item.nextPageCursor || "",
            pageSize: 100, //item.pageSize,
          },
          orderBy: orderBy,
          orderDirection: this.orderDescObj[item.sortDirection],
          parentFileId: "/",
          recursive: true,
          thumbnailStyleList: ["Small", "Big", "Middle", "Large"],
        };
        const reqBack= Utils.sendRequest(requestUrl, body, { type: "json" })
        getCacheDataFn(requestUrl,JSON.stringify(body) ,callback,reqBack,!body.pageInfo.pageCursor);
      }
    },
    // 回收站相关接口
    getVirDirInfo(item, ismountdisk) {
      if (isCutOver() || window.belongsPlatform == 1) {
        var requestUrl = "/hcy/recyclebin/list";
        if (ismountdisk === true) {
          apiDomain = moundDiskUrl;
        }
        var body = {
          pageInfo: {
            pageCursor: item.nextPageCursor || "",
            pageSize: 100, //item.pageSize,
          },
        };
        body = JSON.stringify(body);
        return Utils.sendRequest(requestUrl, body, { type: "json" });
      }
    },
    /**
     * 回收站文件还原
     * @param {boolean} ismountdisk true:挂载盘
     * @returns
     */
    restoreContentsAsync(item, ismountdisk) {
      var requestUrl = "/hcy/recyclebin/batchRestore";
      var requestParams = {
        fileIds: item.fileIds,
      };
      if (ismountdisk === true) {
        requestUrl = "/hcy/recyclebin/batchRestore"; // ToDo: 修改挂载盘接口
      }
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 回收站文件永久删除/清空回收站
    deleteContentsAsync(item) {
      var requestUrl = "/hcy/deleteContentsAsync";
      var requestParams = {
        account: item.account,
        businessType: item.businessType,
        fileIds: item.fileIds,
        directoryIds: item.directoryIds,
        completelyDelete: item.completelyDelete,
      };
      if (ismountdisk === true) {
        requestUrl = "/hcy/deleteContentsAsync"; // ToDo: 修改挂载盘接口
      }
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 新平台回收站文件彻底删除
    batchDelete(item, ismountdisk) {
      var requestUrl = "/hcy/file/batchDelete";
      var requestParams = {
        fileIds: item.fileIds,
      };
      let options = {
        type: "json",
      };
      // 保险箱删除
      if (item.businessType === 4) {
        requestParams.businessType = item.businessType;
        requestParams.sessionId = item.sessionId;
        options.appendHeads = { "x-yun-sbox-session-id": item.sessionId };
      }
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, options);
    },
    // 新平台清空回收站
    recyclebinClear(item, ismountdisk) {
      var requestUrl = "/hcy/recyclebin/clear";
      var requestParams = {};
      if (ismountdisk === true) {
        requestUrl = "/mount/recyclebin/clear"; // ToDo: 修改挂载盘接口
      }
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 回收站查询批量操作任务进度
    queryAsyncTask(taskID, ismountdisk) {
      var requestUrl = "/hcy/task/get";
      var requestParams = {
        taskId: taskID,
      };
      if (ismountdisk === true) {
        requestUrl = "/hcy/mount/task/get"; // ToDo: 修改挂载盘接口
      }
      requestParams = JSON.stringify(requestParams);
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 创建文件夹
    createDirectory(item,ismountdisk = false) {
      if (isCutOver()) {
         var requestUrl = "/hcy/file/create";
        if (ismountdisk) {
          requestUrl = "/mount/file/create"
        }
        var requestParams = {
          // userId: item.userId,
          // parentFileId: item.parentFileId || item.parentId,
          parentPath: item.parentPath,
          fileId: item.fileId,
          name: item.name,
          description: item.description,
          type: "folder", // 创建文件夹
          contentType: item.contentType,
          size: item.size,
          partInfos: item.partInfos,
          expireSec: item.expireSec,
          fileRenameMode: item.fileRenameMode,
          contentHash: item.contentHash,
          contentHashAlgorithm: item.contentHashAlgorithm,
          proofCode: item.proofCode,
          proofVersion: item.proofVersion,
          localCreatedAt: item.localCreatedAt,
          localUpdatedAt: item.localUpdatedAt,
          mediaMetaInfo: item.mediaMetaInfo,
          parallelUpload: item.parallelUpload,
          formUpload: item.formUpload,
          storyVideoFile: item.storyVideoFile,
        };
        if(item.fileRenameMode){
          requestParams.fileRenameMode = 'refuse';
        }
        // 有条件地添加 parentFileId 字段
        if (item.parentFileId || item.parentId) {
          requestParams.parentFileId = (item.parentFileId || item.parentId).includes("00019700101000000001") ? "/" : item.parentFileId || item.parentId;
        }
        requestParams = JSON.stringify(requestParams);
        let options = {
          type: "json",
        };
        if (item.module === "safeBox") {
          let safeSessionInfo = JSON.parse(
            localStorage.getItem("safeSessionInfo")
          );
          if (safeSessionInfo) {
            options.appendHeads = {
              "x-yun-sbox-session-id": safeSessionInfo.sessionID,
            };
          }
        }
        return Utils.sendRequest(requestUrl, requestParams, options);
      }
    },
    // 文件或文件夹重命名
    rename(item, type) {
      if (isCutOver()) {
        var requestUrl = "/hcy/file/update";
        var requestParams = {
          // userId: item.userId,
          fileId: type === "file" ? item.fileId : item.directoryId,
          name: item.name,
          description: item.description,
          FileRenameMode: item.FileRenameMode,
        };
        requestParams = JSON.stringify(requestParams);
        let options = {
          type: "json",
        };
        if (item.module === "safeBox") {
          let safeSessionInfo = JSON.parse(
            localStorage.getItem("safeSessionInfo")
          );
          if (safeSessionInfo) {
            options.appendHeads = {
              "x-yun-sbox-session-id": safeSessionInfo.sessionID,
            };
          }
        }
        return Utils.sendRequest(requestUrl, requestParams, options);
      }
    },
    // 运营类广告位查询
    getAdvertisementInfo(data) {
      var requestUrl =
        "/advertapi/adv-filter/adv-filter/AdInfoFilter/getAdInfos";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 配置类广告位查询
    getAdvertiseConfigInfo(data) {
      let Headers=AdvSignDemo.main()
      // console.log(Headers,'9999999999999999',data,AESUtilDemo.encrypt('18823393859'))
      var requestUrl =
        "/advertapi/adv-config/adv-config/adFilter/getAdInfos";
      return Utils.sendRequest(requestUrl, data, {
        appendHeads: Headers,
        type: "json",
      });
    },
    // 广告位曝光
    pushNewAdInfos(data) {
      var requestUrl =
        "/advertapi/adv-filter/adv-filter/advertReported/reported";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 广告位点击上报
    clickNewAdInfos(data) {
      var requestUrl = "/advertapi/adv-filter/adv-filter/advertReported/click";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 个人云空间大小
    getSpaceSize(data) {
      // var requestUrl = "/user/disk/getPersonalDiskInfo";
      // 新版云空间融合接口
      var requestUrl = "/user/disk/quota/detail";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取文件夹数量
    getFileList(data) {
      var requestUrl = "/hcy/file/statsDir";
      let options = {
        type: "json",
      };
      let params = JSON.parse(data);
      if (params?.sessionId) {
        options.appendHeads = {
          "x-yun-sbox-session-id": params.sessionId,
        };
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取文件路径
    getFileAddress(data) {
      var requestUrl = "/hcy/file/getPath";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
     // 获取文件夹数量
    getMountFileList(data) {
      var requestUrl = "/mount/file/statsDir";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取文件路径
    getFilePath(data) {
      var requestUrl = "/hcy/file/getPath";
      let options = {
        type: "json",
      };
      let params = JSON.parse(data);
      if (params?.sessionId) {
        options.appendHeads = {
          "x-yun-sbox-session-id": params.sessionId,
        };
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取文件下载地址（图片预览-查看原图）
    getDownloadUrl(data) {
      var requestUrl = "/hcy/file/getDownloadUrl";
      let options = {
        type: "json",
      };
      if (data.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete data.isSafeBox;
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取影视资源列表
    getVideoList(data) {
      var requestUrl = "/orchestration/file/video/v1.0/getVideoResourceList";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 影视资源重命名
    movieRename(item) {
      var requestUrl = "/orchestration/file/video/v1.0/updateVideoResource";
      var requestParams = {
        userDomainId: window.userDomainId || window.UserInfo.userDomainId || '',
        fileId: item.fileId,
        name: item.name,
      };
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    // 影视资源删除
    movieDelete(data) {
      var requestUrl = "/orchestration/file/video/v1.0/delVideoResource";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 查询影视资源批量操作进度
    queryMovieTask(data) {
      var requestUrl = "/orchestration/file/task/v1.0/queryTaskDetail";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 上报PC端登录状态（app打开PC）
    uploadPCStatus(data) {
      var requestUrl = "/openapi/interaction/uploadPCStatus";
      var signatureData = Utils.createSignature();
      var options = {
        type: "json",
        appendHeads: {
          "clientId": signatureData.clientId,
          "msgId": signatureData.msgId,
          "signature": signatureData.hash,
          "timestamp": signatureData.timeStamp,
          "version": signatureData.version,
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取文件数据（app打开PC）
    queryFileData(data) {
      var requestUrl = "/openapi/interaction/queryFileData";
      var signatureData = Utils.createSignature();
      var options = {
        type: "json",
        appendHeads: {
          "clientId": signatureData.clientId,
          "msgId": signatureData.msgId,
          "signature": signatureData.hash,
          "timestamp": signatureData.timeStamp,
          "version": signatureData.version,
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取工具箱数据
    getToolBox(data) {
      const requestUrl = "/openapi/toolBox/queryToolBoxInfosByType";
      let signatureData = Utils.createSignature();
      let options = {
        type: "json",
        appendHeads: {
          "clientId": signatureData.clientId,
          "msgId": signatureData.msgId,
          "signature": signatureData.hash,
          "timestamp": signatureData.timeStamp,
          "version": signatureData.version,
          'authType': 1,
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取龙虾实例
    getDragonInstance(data) {
      var requestUrl = "/api/openclaw/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 批量获取文件详情
    getBatchFileDetail(data) {
      var requestUrl = "/hcy/file/batchGet";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },
  group: {
    // 获取文件下载地址（图片预览-查看原图）
    getDownloadUrl(data) {
      var requestUrl =
        "/hcy/mutual/adapter/isbo/openApi/getGroupFileDownLoadURLV2";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取家庭云和共享群视频在线播放地址
    async getFlvOnlineAddr(data, isNonLocal = false) {
      var requestUrl = "/hcy/group/dynamic/assets/getFlvOnlineAddr";
      let options = {
        type: "json",
      };
      options.appendHeads = {
        "api-version": "v2",
      };
      if (isNonLocal) { // 是否请求异网优化资源
        if (!options.appendHeads) {
          options.appendHeads = {};
        }
        options.appendHeads["x-yun-url-type"] = 3;
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 文件云解压
    archiveFile(data) {
      var requestUrl = "/hcy/mutual/adapter/archive/list";
      let options = {
        type: "json",
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 查询文件云解压任务进度和结果
    getArchiveTask(data) {
      var requestUrl = "/hcy/mutual/adapter/archive/task";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 文件云解压到指定目录
    uncompress(data) {
      var requestUrl = "/hcy/mutual/adapter/archive/uncompress";
      let options = {
        type: "json",
      };
      options.appendHeads = {
        "api-version": "v2",
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 文件云解压预览
    previewFiles(data) {
      var requestUrl = "/hcy/mutual/adapter/archive/previewFiles";
      let options = {
        type: "json",
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 文件预览信息获取
    batchGet(data) {
      var requestUrl = "/hcy/group/dynamic/file/batchGetFile";
      let options = {
        type: "json",
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
  },
  family: {
    // 获取文件下载地址（图片预览-查看原图）
    getDownloadUrl(data) {
      var requestUrl =
        "/hcy/family/adapter/andAlbum/openApi/getFileDownLoadURLV2";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    getFamilyList(params) {
      //家庭列表params:{relation:0-创建者1-成员，sequence：0-倒序1正序，默认倒序}
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/queryFamilyCloud";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    queryCloudMember(params) {
      var requestUrl =
        "/hcy/family/adapter/andAlbum/openApi/queryCloudMemberV3";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    getFamilyalbumList(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/queryCloudPhoto";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    getPhotoContents(params) {
      var requestUrl =
        "/hcy/family/adapter/andAlbum/openApi/queryContentInfoV2";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    createFamily(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/createFamilyCloud";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    setFamilyCloudOnTV(params) {
      var requestUrl =
        "/hcy/family/adapter/andAlbum/openApi/hideFamilyCloudOnTV";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    modifyFamilyNick(params) {
      //修改昵称
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/modifyCloudMember";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    modifyCloudFamily(params) {
      //修改群名
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/modifyFamilyCloud";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    deleteFamilyCloud(params) {
      //解散
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/deleteFamilyCloud";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    quitFamilyCloud(params) {
      //退出
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/quitFamilyCloud";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    getWechatInviteQRCode(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/wechatInvitation";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    createInviteQRCode(params) {
      //生成邀请二维码
      var requestUrl = "/mcloud/wechat/api/wxapplet/getacode";
      var options = {
        type: "json",
        responseType: "arraybuffer",
        qrCodeForm: true,
        appendHeads: {
          "x-m4c-token": window.baseToken,
          "x-m4c-account": window.baseuserAccount,
        },
      };
      return Utils.sendRequest(requestUrl, params, options);
    },
    createCloudPhoto(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/createCloudPhoto";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    deleteCloudMember(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/deleteCloudMember";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    queryContentListV3(params) {
      var requestUrl =
        "/hcy/family/adapter/andAlbum/openApi/queryContentListV3";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    getCatalogInfos(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/getCatalogInfos";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    getUserInfo(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/getUserInfo";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    modifyCloudPhoto(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/modifyCloudPhoto";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    deleteCloudPhoto(params) {
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/deleteCloudPhoto";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    modifyContentInfo(params) {
      //文件重命名
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/modifyContentInfo";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    modifyCloudDocV2(params) {
      //文件夹重命名
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/modifyCloudDocV2";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    createCloudDocV2(params) {
      //新建文件夹
      var requestUrl = "/hcy/family/adapter/andAlbum/openApi/createCloudDocV2";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    createBatchOprTaskV2(params) {
      //(批量)任务接口包含转存、删除等
      var requestUrl =
        "/hcy/family/adapter/andAlbum/openApi/createBatchOprTaskV2";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    queryBatchOprTaskV3(params) {
      //查询任务接口
      var requestUrl =
        "/hcy/family/adapter/andAlbum/openApi/queryBatchOprTaskDetailV3";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
  },
  note: {
    noteInit(params) {
      var requestUrl = "/sync/all/init";
      return Utils.sendRequest(requestUrl, {});
    },
    getNotebook(params) {
      var requestUrl = "/sync/all/execute";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    getNoteDetail(params) {
      var requestUrl = "/noteServer/api/getNote.do";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    queryNoteTemplates(params,Headers) {
      var requestUrl = "/cloudNote/api/template/queryAllDefaultTemplate";
      console.log("queryNoteTemplates888", params,Headers);
      return Utils.extraHeadersRequset(requestUrl, params, {
        appendHeads: Headers,
      });
    },
    translateTitle(params, Headers) {
      //标题翻译
      var requestUrl = "/csbo/api/common/translate";
      var appendHeads = {
        "x-m4c-caller": "platform",
        "x-m4c-src": "10007",
        //"Connection":"keep-alive",
        "x-m4c-token": Headers?.NOTE_TOKEN || window?.userInfo?.token,
        // "x-m4c-account": window.userDomainId || window.UserInfo.userDomainId || '',
      };
      Headers = { ...appendHeads, ...Headers };
      return Utils.extraHeadersRequset(requestUrl, params, {
        appendHeads: Headers,
      });
    },
    longTranslateFn(params, Headers) {
      //内容翻译
      var requestUrl = "/csbo/api/common/longTranslate";
      var appendHeads = {
        "x-m4c-caller": "platform",
        "x-m4c-src": "10007",
        //"Connection":"keep-alive",
        "x-m4c-token": Headers?.NOTE_TOKEN || window?.userInfo?.token,
        // "x-m4c-account": window.account || window?.userInfo?.account,
      };
      Headers = { ...appendHeads, ...Headers };
      return Utils.extraHeadersRequset(requestUrl, params, {
        appendHeads: Headers,
        timeout:180000
      });
    },
    getTranslateResultFn(params, Headers) {
      //翻译结果
      var requestUrl = "/csbo/api/common/getTranslateResult";
      var appendHeads = {
        "x-m4c-caller": "platform",
        "x-m4c-src": "10007",
        //"Connection":"keep-alive",
        "x-m4c-token": Headers?.NOTE_TOKEN || window?.userInfo?.token,
        // "x-m4c-account": window.account || window?.userInfo?.account,
      };
      Headers = { ...appendHeads, ...Headers };
      return Utils.extraHeadersRequset(requestUrl, params, {
        appendHeads: Headers,
      });
    },
    fileSaveToNote(params, Headers) {
      //上传附件
      var requestUrl = "/yun-note/note/fileSaveToNote";
      return Utils.extraHeadersRequset(requestUrl, params, {
        appendHeads: Headers,
      });
    },
    // 笔记分享列表（与文件分享接口一致，走 sendRequest 自动从 UserInfo 带头）
    listNoteShare(params) {
      var requestUrl = "/share/api/note/listNoteShare.do";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    // 获取笔记分享信息
    getNoteShareinfo(params) {
      var requestUrl = "/share/api/note/getNoteShareinfo.do";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    // 取消笔记分享
    cancelNoteShareV2(params) {
      var requestUrl = "/share/api/note/cancelNoteShareV2.do";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    //获取AI白名单
    getAiWhite(params){
      let uuid = Utils.generateUUID();
      let signature = Utils.cloudDynamicAuthentication(uuid);
      let timeStamp = new Date().getTime();
      let options = {
        type: "json",
        appendHeads: {
          msgId: uuid,
          signature: signature,
          version: "1.1",
          timeStamp: timeStamp,
          clientId: "pcapp",
          authType: 1,
        },
      };
      var requestUrl = "/openapi/general/getStatus";
      return Utils.sendRequest(requestUrl, params, options);
    },
    //获取鼠标权益
    getMouseAuth(params){
      var options = {
        type: "orginXml",
        appendHeads: {
          "x-yun-userDomainId": "enabled"
        },
      };
      var requestUrl = "/cloudSEE/openApi/queryRightsSubscribeRelation";
      return Utils.sendRequest(requestUrl, params, options);
    }
  },
  dynamic: {
    getCloudList(item) {
      var requestUrl, requestParams, type;
      type = "json";
      requestUrl = "/openapi/pDynamicInfo/queryBatchList";
      requestParams = JSON.stringify(item);
      return Utils.sendRequest(requestUrl, requestParams, { type });
    },
    getCloudListInfo(item) {
      var requestUrl, requestParams, type;
      type = "json";
      requestUrl = "/openapi/pDynamicInfo/queryBatchInfo";
      requestParams = JSON.stringify(item);
      return Utils.sendRequest(requestUrl, requestParams, { type });
    },
    clearCloudList(item) {
      var requestUrl, requestParams, type;
      type = "json";
      requestUrl = "/openapi/pDynamicInfo/removeByInfoIds";
      requestParams = JSON.stringify(item);
      return Utils.sendRequest(requestUrl, requestParams, { type });
    },
    clearContentList(item) {
      var requestUrl, requestParams, type;
      type = "json";
      requestUrl = "/openapi/pDynamicInfo/removeContents";
      requestParams = JSON.stringify(item);
      return Utils.sendRequest(requestUrl, requestParams, { type });
    },
  },
  search: {
    // 共享群组列表
    getGroupList(data) {
      var requestUrl = "/hcy/mutual/adapter/search/sharingSearch-ss001";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 共享群组文件搜索
    getGroupFileList(data) {
      var requestUrl = "/hcy/mutual/adapter/search/sharingSearch-ss003";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 共享群文件类型搜索
    getGroupFileByType(data) {
      var requestUrl = "/hcy/mutual/adapter/search/sharingSearch-ss004";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 个人云/同步盘文件搜索
    getCloudList(data) {
      var requestUrl = "/search/SearchFile";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 家庭云相册搜索
    getFamilyPhoto(data) {
      var requestUrl = "/hcy/family/adapter/search/famSearch-fs001";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 家庭云文件搜索
    getFamilyFile(data) {
      var requestUrl = "/hcy/family/adapter/search/famSearch-fs003";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 家庭云通过类型搜索(图片类型)
    getFamilyFileByPhoto(data) {
      var requestUrl = "/hcy/family/adapter/search/famSearch-fs007";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 家庭云类型（除照片类型）搜索
    getFamilyFileByType(data) {
      var requestUrl = "/hcy/family/adapter/search/famSearch-fs004";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },
  file: {
    // 共享群文件批量操作、个人云转存
    groupFileAction(data) {
      var requestUrl = "/hcy/mutual/adapter/isbo/openApi/createBatchOprTask";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 共享群文件批量操作进度查询
    groupFileProgress(data) {
      var requestUrl =
        "/hcy/mutual/adapter/isbo/openApi/queryBatchOprTaskDetailV3";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 个人云文件批量操作进度查询
    personFileProgress(data) {
      var requestUrl = "/hcy/task/get";
      let options = {
        type: "json",
      };
      if (data.sessionId) {
        options.appendHeads = {
          "x-yun-sbox-session-id": data.sessionId,
        };
        // 缓存页面是保险箱页面
        window.routerName = "safeBoxList";
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取常用目录列表
    getCommonCatalogList(data) {
      var requestUrl = "/openapi/commonPath/getCommonPathByPhone";
      return Utils.sendRequest(requestUrl, data, {
        type: "json",
        appendHeads: { authType: 1 },
      });
    },
    // 个人云文件删除
    delPersonFile(data) {
      var requestUrl = "/hcy/recyclebin/batchTrash";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 个人云文件移动
    movePersonFile(data) {
      var requestUrl = "/hcy/file/batchMove";
      let options = {
        type: "json",
      };
      if (data.module === "safeBox") {
        let safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
          // 缓存页面是保险箱页面
          window.routerName = "safeBoxList";
          delete data.module;
        }
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 个人云文件复制
    copyPersonFile(data) {
      var requestUrl = "/hcy/file/batchCopy";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 文件云解压
    archiveFile(data,ismountdisk = false) {
      var requestUrl = "/hcy/archive/list";
      if (ismountdisk) {
        requestUrl = "/mount/archive/list"
      }
      let options = {
        type: "json",
      };
      if (data.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete data.isSafeBox;
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 查询文件云解压任务进度和结果
    getArchiveTask(data,ismountdisk = false) {
      var requestUrl = "/hcy/archive/getTask";
      if (ismountdisk) {
        requestUrl = "/mount/archive/getTask";
      }
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 文件云解压到指定目录
    uncompress(data,isMountdisk = false) {
      var requestUrl = "/hcy/archive/uncompress";
       if (isMountdisk) {
        requestUrl = "/mount/archive/uncompress"
      }
      let options = {
        type: "json",
      };
      if (data.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete data.isSafeBox;
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 文件云解压预览
    previewFiles(data,isMountdisk = false) {
      var requestUrl = "/hcy/archive/previewFiles";
      if (isMountdisk) {
        requestUrl = "/mount/archive/previewFiles"
      }
      let options = {
        type: "json",
      };
      if (data.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete data.isSafeBox;
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 文件预览信息获取
    batchGet(data) {
      var requestUrl = "/hcy/file/batchGet";
      let options = {
        type: "json",
      };
      if (data.isSafeBox) {
        var safeSessionInfo = JSON.parse(
          localStorage.getItem("safeSessionInfo")
        );
        if (safeSessionInfo) {
          options.appendHeads = {
            "x-yun-sbox-session-id": safeSessionInfo.sessionID,
          };
        }
        delete data.isSafeBox;
      }
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 获取文件详情
    getFileInfo(data) {
      var requestUrl = "/hcy/file/get";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 获取文件路径详情
    getFilePathInfo(data) {
      var requestUrl = "/hcy/file/getPath";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 查找pdf文件转换记录
    convertFile(data) {
      var requestUrl = "/hcy/file/online/wps/convertFile";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 创建pdf文件转换任务
    convertPdf(data) {
      var requestUrl = "/hcy/file/online/wps/useBenefit/convertPdf";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 查询pdf文件转换任务进度
    convertProcess(data) {
      var requestUrl = "/hcy/file/online/wps/convertProcess";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },
  messageCenter: {
    queryMessageCenterInfo(data, path) {
      console.log("messageCenter queryMessageCenterInfo>" + path);
      var requestUrl = path;
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },
  share: {
    // 获取分享列表
    getShareList(data) {
      var requestUrl = "/yun-share/richlifeApp/devapp/IOutLink";
      var options = {
        appendHeads: {
          "x-yun-userDomainId": "enabled"
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 取消分享
    cancelShare(data) {
      var requestUrl = "/yun-share/richlifeApp/devapp/IOutLink";
      var options = {
        type: "orginXml",
        appendHeads: {
          "x-yun-userDomainId": "enabled"
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 分享文件
    shareFile(encrypt, sharetime, pwdFillType, files, dirs) {
      var requestUrl = "/yun-share/richlifeApp/devapp/IOutLink";
      var options = {
        type: "orginXml",
        appendHeads: {
          "x-yun-userDomainId": "enabled"
        },
      };
      var channel = "3003";
      var getShareItemId = (item) => item?.id || item?.contentID || item?.fileId || "";
      var fileIds = [];
      (files || []).forEach((item) => {
        var fileId = getShareItemId(item);
        if (fileId) {
          fileIds.push(fileId);
        }
      });
      var dirIds = [];
      (dirs || []).forEach((item) => {
        var dirId = getShareItemId(item);
        if (dirId) {
          dirIds.push(dirId);
        }
      });
      var filesCount = fileIds.length;
      var dirsCount = dirIds.length;
      var fileXml = fileIds.map((id) => "<item>" + id + "</item>").join("");
      var dirXml = dirIds.map((id) => "<item>" + id + "</item>").join("");
      var body = `<?xml version='1.0' encoding='utf-8'?>
                <getOutLinkV3>
                <getOutLinkReq>
                <userDomainId>${window.userDomainId || window.UserInfo.userDomainId || ''}</userDomainId>
                <linkType>0</linkType>
                <pubType>1</pubType>
                <encrypt>${encrypt}</encrypt>
                <period>${sharetime}</period>
                <passwdAutoFill>${pwdFillType}</passwdAutoFill>
                <coIDLst length="${filesCount}">
                ${fileXml}
                </coIDLst>
                <caIDLst length="${dirsCount}">
                ${dirXml}
                </caIDLst>
                <extInfo size="1">
                <item>
                <key>shareChannel</key>
                <value>${channel}</value>
                </item>
                </extInfo>
                </getOutLinkReq>
                </getOutLinkV3>`;
      return Utils.sendRequest(requestUrl, body, options);
    },
    // 生成分享二维码
    getQRcode(data) {
      const Buffer = require("buffer").Buffer;
      var account = Buffer.from(window.UserInfo.account);
      var authToken = Buffer.from(window.UserInfo.authToken);
      var base64_account = account.toString("base64");
      var base64_authToken = authToken.toString("base64");
      var requestUrl =
        "/orchestration/thirdPart/wechat/v1.0/miniProgram/createQRCode"; // todo--无法使用vue.config.js中的代理，原因不明，暂时使用完整接口地址
      var options = {
        type: "json",
        appendHeads: {
          "x-m4c-account": base64_account,
          "x-m4c-token": base64_authToken,
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    },
    // 查询管控开关（白名单）
    queryControlSwitch(data) {
      var requestUrl = "/yun-share/richlifeApp/devapp/IControl";
      var options = {
        appendHeads: {
          "x-yun-userDomainId": "enabled"
        },
      };
      return Utils.sendRequest(requestUrl, data, options);
    }
  },
  //通讯录
  addressBookManagement: {
    getRsaPublicKey(data) {
      var requestUrl = "/caiyun/openapi/authentication/key/getRsaPublicKey";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },
  skin: {
    getSkinList(data) {
      let uuid = Utils.generateUUID();
      let signature = Utils.cloudDynamicAuthentication(uuid);
      let requestUrl = "/openapi/skinConfig/queryV2";
      let timeStamp = new Date().getTime();
      let options = {
        type: "json",
        appendHeads: {
          msgId: uuid,
          signature: signature,
          version: "1.1",
          timeStamp: timeStamp,
          clientId: "pcapp",
          authType: 1,
        },
      };
      console.log("换肤 getSkinList head:" + options);
      return Utils.sendRequest(requestUrl, data, options);
    },
    //哀悼皮肤
    getGraySkin(data) {
      var requestUrl = "/openapi/grayConfig/query";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },
  upgrade: {
    get139mailUpgradeInfo(data) {
      var requestUrl =
        "http://appupdate.mail.10086.cn/AppClientUpgrade/upgradeCheck.do";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    //检查更新
    checkUpgrade(data) {
      //todo，跨域问题。
      var requestUrl = "/user/version/check";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },
  mountApi:{
     mountProgress(data) {
      var requestUrl = "/mount/task/get";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
     mountTransTaskProgress(data) {
      var requestUrl = "/mount/transTask/get";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
      mountDelete(data) {
      var requestUrl = "/mount/recyclebin/batchTrash";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    mountSerch(data) {
      var requestUrl = "/mount/file/search";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    //转存
    mountTrans(data){
      var requestUrl = "/mount/transTask/create";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    mountMove(data){
      var requestUrl = "/mount/file/batchMove";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    mountRename(data,type){
        var requestParams = {
          fileId: type === "file" ? data.fileId : data.directoryId,
          name: data.name,
          description: data.description,
          FileRenameMode: data.FileRenameMode,
        };
      var requestUrl = "/mount/file/update";
      return Utils.sendRequest(requestUrl, requestParams, { type: "json" });
    },
    mountSearch(data){
      var requestUrl = "/search/SearchFileMountingDisc";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    //回收站列表
    mountRecycleList(item){
      let data = {
        pageInfo: {
          pageCursor: item.nextPageCursor || "",
          pageSize: 20,
        },
      };
      var requestUrl = "/mount/recyclebin/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    mountRestore(item){
     var data = {
        fileIds: item.fileIds,
      }
      var requestUrl = "/mount/recyclebin/batchRestore";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    //永久删除
    mountBatchDelete(item){
     var data = {
        fileIds: item.fileIds,
      }
      var requestUrl = "/mount/file/batchDelete";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    mountHistory(params) {
      var requestUrl = "/mount/revision/listRevision";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    mountGetDownloadUrl(params) {
      var requestUrl = "/mount/file/getDownloadUrl";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    mountDeleteHistoryList(params) {
      var requestUrl = "/mount/revision/deleteRevision";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
    mountRestoreRevision(params) {
      var requestUrl = "/mount/revision/restoreRevision";
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    },
  },
  // 知识库
  knowledgeBase: {
    // 创建个人知识库
    createKnowledgeBase(data) {
      var requestUrl = "/api/outer/assistant/knowledge/personal/v2/base/create";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 查询个人知识库列表接口
    queryKnowledgeBaseList(data) {
      var requestUrl = "/api/outer/assistant/knowledge/personal/v2/base/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 个人知识库批量导入资源接口
    batchImport(data) {
      var requestUrl = "/api/outer/assistant/knowledge/personal/v2/base/file/batchImport";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 查询个人知识库导入任务列表接口
    queryImportTaskList(data) {
      var requestUrl = "/api/outer/assistant/knowledge/personal/v2/base/importTask/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
    // 查询个人知识库资源列表接口
    queryKnowledgeBaseResourceList(data) {
      var requestUrl = "/api/outer/assistant/knowledge/personal/v2/base/resource/list";
      return Utils.sendRequest(requestUrl, data, { type: "json" });
    },
  },

  // 在线预览
  onlineFile: {
    onlineFile(params) {
      var requestUrl = "/hcy/file/online/file/getWpsUrl";
      params.userId = window.UserInfo.userDomainId;
      return Utils.sendRequest(requestUrl, params, { type: "json" });
    }
  },
};
