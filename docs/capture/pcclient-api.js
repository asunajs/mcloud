console.log('************api.js')
//自研埋点获取渠道号
function getChannelInformation(callback) {
  jsonRequestMission("getWindowStatus", null, result => callback && callback(result))
}

function openWindow(url, width, height) {
  if (!width) {
    width = 1024;
  }
  if (!height) {
    height = 768;
  }
  jsonRequestMission("openWebsite", {
    userId: window.userId,
    url: url,
    type: 2
  }, () => { })
}

function userLogin(account, pass, loginMode) {
  if (account.indexOf("@")) {
    account = account.split("@")[0]
  }
  var token = pass;



  var arr = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    "<object>",
    '<string name="clientId">1003</string>',
    '<string name="version">66</string>',
    '<string name="loginType">', loginMode > 0 ? "4" : "0", '</string>',
    // '<string name="autoSecreKey">1</string>',
    '<string name="createAutoLoginSecretKey">0</string>',
    //'<string name="verifyCode">""</string>',
    '<string name="authType">', loginMode > 0 ? "0" : "2", '</string>',
    '<string name="needWCookie">1</string>',
    //'<string name="verifyAgentId"></string>',
    '<string name="loginName">' + account + '</string>',
    '<string name="loginPassword">' + token + '</string>',
    "</object>"
  ];

  //netType 上传网络类型0：4G, 1：wifi, 2:未知window.location.protocol
  var cguid = Math.random().toString(16).replace(".", "");
  var url = "https://mail.10086.cn/login/inlogin.action?func=inlogin.action&cguid=" + cguid;
  return httpRequest(url, arr.join(""))


}

window.behaviorList = [];

function BH(actionId, thingId) {
  logBehavior({
    actionId,
    thingId
  });
}

function logBehavior(options) {
  var item = {};
  if (options.actionId) item.action = String(options.actionId);
  if (options.thingId) item.thingId = String(options.thingId);
  item.module = String(options.moduleId || 25);
  item.actionType = String(options.actionType || 20);
  item.pageId = String(options.pageId || 24);

  //插到待发队列里
  window.behaviorList.push(item);


}

//初始化行为统计监听
document.addEventListener("DOMContentLoaded", function () {
  if (!window.behaviorTimer) {
    document.addEventListener("click", function (e) {
      var target = e.target;
      behaviorClick(target);

      function behaviorClick(target) {

        var element = target;
        var bhKey;
        while (element) {
          if (element.getAttribute) {
            bhKey = element.getAttribute("bh") || element.getAttribute("behavior");
          }
          if (bhKey) {
            var arr = bhKey.split(",");
            BH.apply(this, arr);
            break;
          } else {
            element = element.parentNode;
          }
        }
      }
    });
    // window.behaviorTimer = setInterval(function () {
    //   // 
    //   if (window.behaviorList.length > 0) {
    //     var list = window.behaviorList.concat();
    //     researchRequest({
    //       version: "m2012",
    //       behaviors: list
    //     }, json => {
    //       // console.log(json);
    //     })
    //     window.behaviorList = [];
    //   }
    // }, 3000)
  }

  window.onerror = function (msg, source, line, col, error) {

    // throw msg
    var errorMsg = "[JsError]Error=" + msg + " | Line=" + line + " | Column=" + col + " | Stack=" + (error && error.stack);
    writeLog(2, 0, errorMsg)
    return true
  }


})


function httpError(url, response) {
  var errorMsg = "[HttpError]Url=" + url + " | Response=" + response;
  writeLog(2, 0, errorMsg)
}

function writeLog(level, whichFile, msg) {
  if(msg == undefined){
    msg = "";
  }
  window.nativeAbility.logger.info(msg);
  // console.error("writeLog尚未实现"); 
  // return ;
  // apiNative.common.writeLog(msg);
  // 日志打印位置 whichFile: 0 main.log  1 note.log
  // jsonRequestMission("writeLog", {
  //   "level": level,
  //   "whichFile": whichFile,
  //   msg: msg
  // }, result => {
  //   // console.log("writeLog",result,'\n',msg,"msg")
  // })
}

function setDefaultAccount(uid) {
  jsonRequestMission("setAccount", {
    "userId": uid
  }, () => { })
}

function checkSyncDiskStatus(dirId, callback) {
  jsonRequestMission("synchronousDiskStatus", {
    "userId": window.userId,
    dirId: dirId
  }, function (result) {
    if (result.code == "S_OK") {
      //status 0：未开启；1：暂停；2：正在同步；3：空闲
      callback(result["var"]);
    }
  })
}


function settingPath(callback, data) {
  jsonRequestMission("openSyncGuide", {
    account: window.userAccount,
    ...data,
  }, function (result) {
    callback(result);
  })
}

// 设置同步盘状态
function setSyncDiskState(callback, data) {
  /**
   * operation int 1：开启同步盘 2：暂停同步盘 3: 重置同步盘 
   * syncDir string 同步盘目录， operation为1时有效, 暂时传空字符
   */
  console.log('++++data', data)
  jsonRequestMission("setSyncDiskStateNSD", {
    account: window.userAccount,
    ...data
  }, function (result) {
    callback(result);
  })
}

function editSyncPath(callback, data) {
  jsonRequestMission("modifySyncGuidePath", {
    account: window.userAccount,
    ...data,
  }, function (result) {
    callback(result);
  })
}
//8.4.0新增接口-查询用户(个人云)目录下是否存在同名的"同步盘"文件或文件夹
function queryCloudHasSameSyncFile(callback, data) {
    console.log("入参",window.userAccount);
    jsonRequestMission("getCloudSyncDiskDirInfo", {
      account: window.userAccount
    }, function (result) {console.log("getCloudSyncDiskDirInfo 响应》》",result);
      callback(result);
    })
  }
//新版同步盘
// 1 开启  0 不启用
function SyncManage(dirId, operation, callback) {
  jsonRequestMission("onOffNSD", {
    account: window.userAccount,
    operation: operation
  }, function (result) {
    //console.log(result,"新版同步盘开启");
    callback(result["var"]);
  })
}

//1: 最大化窗口，2: 最小化窗口, 3: 还原窗口, 4: 锁定客户端 5:关闭 6:打开login窗口
function TitleCommand(cmd, type) {//type打开窗口时的id(type),主窗口'main'
  var map = {
    max: 1,//最大化
    min: 2,//最小化
    restore: 3,//还原
    //lock: 4,
    close: 4,
    allScreen:5,//全屏
    login: 6,
    relogin: 7,
    skin:8
  }
   console.log("cccmd2",type,map[cmd],window.nativeAbility);
   const _controls =window.nativeAbility.controls;
   _controls&&_controls.windowOperate(type,map[cmd]);
}
function getPublicUrlSi(str, storageStr = "cutoverInfo", from) {//commmon/tools.js里有这个类似方法，但import进来页面会报错，再重装一下
  let obj = getStorage(storageStr);
  let targetUrl
  if (!obj || !obj.routerAddr || (obj.cutoverStatus == 0 && !from)) {
    targetUrl = ''
  } else {
    let targetObj = obj.routerAddr.find(item => item.modName == str);
    targetUrl = targetObj.httpsUrl
  }
  function getStorage(key) {
    let value = localStorage.getItem(key);
    try {
      value = JSON.parse(value);
    } catch (error) { value = ""; }
    return value;
  }
  return targetUrl
}

var OutLink = {
  createItems: function (encrypt, sharetime, files, dirs, callback) {
    window.cutoverInfo_shared = window.cutoverInfo_shared ? window.cutoverInfo_shared : getPublicUrlSi('shared') ? getPublicUrlSi('shared') : "";
    window.personalCloudUrl = window.personalCloudUrl ? window.personalCloudUrl : getPublicUrlSi('personal') ? getPublicUrlSi('personal') : "";
    var shareDomainUrl = window.cutoverInfo_shared ? window.cutoverInfo_shared : "";
    var userId = window.userId;
    var userAccount = window.userAccount;

    var filesCount = files.length;
    fileXml = "";
    files.forEach((item) => {
      fileXml += "<item>" + item.id + "</item>"
    })

    var dirsCount = dirs.length;
    dirXml = "";
    dirs.forEach((item) => {
      dirXml += "<item>" + item.id + "</item>"
    })

    var channel = '3003';
    var body = `<?xml version='1.0' encoding='utf-8'?> 
                <getOutLinkV3> 
                <getOutLinkReq> 
                <account>${userAccount}</account> 
                <linkType>0</linkType> 
                <pubType>1</pubType>
                <encrypt>${encrypt}</encrypt>
                <period>${sharetime}</period>
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
                </getOutLinkV3>`

    if (personalCloudUrl) {
      body = `<?xml version='1.0' encoding='utf-8'?> 
                <getOutLinkV3> 
                <getOutLinkReq> 
                <account>${userAccount}</account> 
                <linkType>0</linkType> 
                <pubType>1</pubType>
                <encrypt>${encrypt}</encrypt>
                <period>${sharetime}</period>
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
                </getOutLinkV3>`
      transparentTransmission('IOutLink', body, null, 'getoutlinkv3', json => {
        if (json.code === 'S_OK') {
          callback(json.data.result.getOutLinkRes.getOutLinkResSet.getOutLinkResOne)
        } else {
          callback(json);
          const callbackCode = ["9470", "200000768", "200000767"]
          if (callbackCode.includes(json.data.resultCode)) {
            return
          } else {
            if (json.data.resultCode == "200000507") {
              toast(json.data.desc);
              return;
            }
            toast(`${errcodeList['' + json.data.resultCode] || '网络异常，请检查网络后重试'}(${json.data.resultCode})`)
          }
        }
      }, true, true, true, `${shareDomainUrl}/richlifeApp/devapp`, 'application/xml')//''
    }
    else {
      transparentTransmission('IOutLink', body, null, 'getoutlinkv3', json => {
        if (json.code === 'S_OK') {
          callback(json.data.result.getOutLinkRes.getOutLinkResSet.getOutLinkResOne)
        } else {
          callback(json);
          const callbackCode = ["9470", "200000768", "200000767"]
          if (callbackCode.includes(json.data.resultCode)) {
            return
          } else {
            toast(`${errcodeList['' + json.data.resultCode] || '网络异常，请检查网络后重试'}(${json.data.resultCode})`)
          }
        }
      })
    }

  },
  //获取小程序二维码
  createQRCode(account, token, form, callback) {
    var head = `transparentTransmission
url:http://csbo.mcloud.139.com/csbo/api/wechat/getacode
response2base64:true
POST csbo/api/wechat/getacode HTTP/1.1
Host:csbo.mcloud.139.com
x-m4c-token:${token} 
x-m4c-account:${account}
Accept:text/html, image/gif, image/jpeg, *; q=.2, */*; q=.2
Content-Type:application/x-www-form-urlencoded
responseType:arraybuffer`

    //请求次数
    // ++window.timeIndex    
    var body = `${form}`
    var content = head + "\n\n" + body;
    //请求次数
    ++window.timeIndex
    window.QRCodeIndex.push(window.timeIndex)
    window.bridge.h5request(window.timeIndex, content);
    //记录保存请求函数对应callback
    window.resultList[window.timeIndex] = callback;
  },

  //获取小程序二维码
  createQRCodePds(account, token, form, callback) {
    var head;
    let userAccount = window.userAccount;
    if (window && window.systemEnvironment && window.systemEnvironment === '22') {
      head = `transparentTransmission
url:https://miniprogram.mail.10086.cn/orchestration-ecloud-tiyan/thirdPart/wechat/v1.0/miniProgram/createQRCode
POST orchestration-ecloud-tiyan/thirdPart/wechat/v1.0/miniProgram/createQRCode HTTP/1.1
Host:miniprogram.mail.10086.cn
account:${userAccount}
x-m4c-token:${token} 
x-m4c-account:${account}
Accept:text/html, image/gif, image/jpeg, *; q=.2, */*; q=.2
Content-Type:application/x-www-form-urlencoded
Content-Type:application/json;charset=utf-8`
    } else {
      head = `transparentTransmission
url:https://orches.yun.139.com/orchestration/thirdPart/wechat/v1.0/miniProgram/createQRCode
POST orchestration/thirdPart/wechat/v1.0/miniProgram/createQRCode HTTP/1.1
Host:miniprogram.mail.10086.cn
x-m4c-token:${token} 
x-m4c-account:${account}
Accept:text/html, image/gif, image/jpeg, *; q=.2, */*; q=.2
Content-Type:application/x-www-form-urlencoded
Content-Type:application/json;charset=utf-8`
    }

    //请求次数
    // ++window.timeIndex    
    var body = `${form}`
    var content = head + "\n\n" + body;
    let requestName = "createQRCode";
    let isPDS = personalCloudUrl ? true : false;
    //请求次数
    ++window.timeIndex
    window.QRCodeIndex.push(window.timeIndex)
    window.bridge.h5request(window.timeIndex, content, isPDS, requestName);
    //记录保存请求函数对应callback
    window.resultList[window.timeIndex] = callback;
  },

  getInfo(linkId, callback) {
    window.cutoverInfo_shared = window.cutoverInfo_shared ? window.cutoverInfo_shared : getPublicUrlSi('shared') ? getPublicUrlSi('shared') : "";
    window.personalCloudUrl = window.personalCloudUrl ? window.personalCloudUrl : getPublicUrlSi('personal') ? getPublicUrlSi('personal') : "";
    var shareDomainUrl = window.cutoverInfo_shared ? window.cutoverInfo_shared : "";
    var body = `<?xml version='1.0' encoding='utf-8'?> 
                <getOutLinkInfo> 
                <getOutLinkInfoReq> 
                <account>${userAccount}</account> 
                <linkID>${linkId}</linkID> 
                <pCaID>root</pCaID> 
                <caSrt>0</caSrt> 
                <coSrt>0</coSrt> 
                <srtDr>1</srtDr> 
                <bNum>1</bNum> 
                <eNum>2</eNum> 
                </getOutLinkInfoReq> 
                </getOutLinkInfo>`

    if (personalCloudUrl) {
      transparentTransmission('IOutLink', body, false, "getoutlinkinfo", result => {
        if (result.code === "S_OK") {
          callback(result.data)
        }
      }, true, true, true, `${shareDomainUrl}/richlifeApp/devapp`, 'application/xml')//''
    }
    else {
      transparentTransmission('IOutLink', body, false, "getoutlinkinfo", result => {
        if (result.code === "S_OK") {
          callback(result.data)
        }
      })
    }

  },
  getList(pageIndex, pageSize, callback) {
    window.cutoverInfo_shared = window.cutoverInfo_shared ? window.cutoverInfo_shared : getPublicUrlSi('shared') ? getPublicUrlSi('shared') : "";
    window.personalCloudUrl = window.personalCloudUrl ? window.personalCloudUrl : getPublicUrlSi('personal') ? getPublicUrlSi('personal') : "";
    var shareDomainUrl = window.cutoverInfo_shared ? window.cutoverInfo_shared : "";
    var body = `<?xml version='1.0' encoding='utf-8'?> 
    <getOutLinkLstV2> 
    <getOutLinkLstReq> 
    <account>${userAccount}</account> 
    <linkType>0</linkType> 
    <srt>1</srt> 
    <srtDr>0</srtDr> 
    <bNum>${pageIndex}</bNum> 
    <eNum>${pageSize}</eNum> 
    <qryType>1</qryType>
    </getOutLinkLstReq> 
    </getOutLinkLstV2>`


    if (personalCloudUrl) {
      transparentTransmission('IOutLink', body, false, "getOutLinkLstV2", result => {
        console.log(JSON.parse(JSON.stringify(result)))
        if (result.code === "S_OK") {
          let { outLinks, count } = result.data.result.getOutLinkLstV2Res
          if (!Array.isArray(outLinks)) {
            outLinks = [outLinks]
          }
          let arr = []
          outLinks.forEach(item => {
            if (item && item.outLinkV2 && item.outLinkV2.isOutLinkInvalid && item.outLinkV2.isOutLinkInvalid !== '1') {
              arr.push(item.outLinkV2)
            }
          })
          //count = arr.length;
          callback(arr, count)
        } else {
          callback(result);
          toast(`${errcodeList['' + result.data.resultCode] || '网络异常，请检查网络后重试'}(${result.data.resultCode})`)
        }
      }, true, true, true, `${shareDomainUrl}/richlifeApp/devapp`, 'application/xml') //''     
    }
    else {
      transparentTransmission('IOutLink', body, false, "getOutLinkLstV2", result => {
        console.log(JSON.parse(JSON.stringify(result)))
        if (result.code === "S_OK") {
          let { outLinks, count } = result.data.result.getOutLinkLstV2Res
          if (!Array.isArray(outLinks)) {
            outLinks = [outLinks]
          }
          let arr = []
          outLinks.forEach(item => {
            if (item && item.outLinkV2 && item.outLinkV2.isOutLinkInvalid && item.outLinkV2.isOutLinkInvalid !== '1') {
              arr.push(item.outLinkV2)
            }
          })
          // 这里之前人改的不对，添加之后会导致只请求第一页
          // count = arr.length;
          callback(arr, count)
        } else {
          callback(result);
          toast(`${errcodeList['' + result.data.resultCode] || '网络异常，请检查网络后重试'}(${result.data.resultCode})`)
        }
      })
    }

  },
  deleteItems: function (items, callback) {
    window.cutoverInfo_shared = window.cutoverInfo_shared ? window.cutoverInfo_shared : getPublicUrlSi('shared') ? getPublicUrlSi('shared') : "";
    window.personalCloudUrl = window.personalCloudUrl ? window.personalCloudUrl : getPublicUrlSi('personal') ? getPublicUrlSi('personal') : "";
    var shareDomainUrl = window.cutoverInfo_shared ? window.cutoverInfo_shared : "";
    var count = items.length;
    itemXml = "";
    items.forEach((item) => {
      itemXml += "<item>" + item.linkID + "</item>"
    })
    var body = `<?xml version='1.0' encoding='utf-8'?> 
                <delOutLink> 
                <delOutLinkReq> 
                <account>${userAccount}</account> 
                <linkIDs length="${count}"> 
                ${itemXml}
                </linkIDs> 
                </delOutLinkReq> 
                </delOutLink>`

    if (personalCloudUrl) {
      transparentTransmission('IOutLink', body, false, "deloutlink", result => {
        if (result.code !== "S_OK") {
          if (result.data.resultCode == '222000400') {
            toast("单次取消分享数量达到上限")
          } else {
            toast(`${errcodeList['' + result.data.resultCode] || '网络异常，请检查网络后重试'}(${result.data.resultCode})`)
          }
        }
        callback(result)
      }, true, true, true, `${shareDomainUrl}/richlifeApp/devapp`, 'application/xml')//''
    }
    else {
      transparentTransmission('IOutLink', body, false, "deloutlink", result => {
        if (result.code !== "S_OK") {
          if (result.data.resultCode == '222000400') {
            toast("单次取消分享数量达到上限")
          } else {
            toast(`${errcodeList['' + result.data.resultCode] || '网络异常，请检查网络后重试'}(${result.data.resultCode})`)
          }
        }
        callback(result)
      })
    }

  }
}

/* 隐私协议确认 */
var userPrivacyAgreement = (params) => new Promise((resolve, reject) => {
  var body = `<?xml version='1.0' encoding='utf-8'?>
              <userPrivacyAgreement>
                  <userPrivacyAgreementReq>
                      <account>${params.userAccount || window.userAccount}</account>
                      <confirmType>${params.confirmType}</confirmType> 
                      <forceConfirm>${params.forceConfirm}</forceConfirm>
                  </userPrivacyAgreementReq>
              </userPrivacyAgreement>`
  transparentTransmission('bmsuite.IUser', body, null, 'userprivacyagreement', json => {
    if (json.code === 'S_OK') {
      resolve(json)
    } else {
      toast(`${errcodeList['' + json.data.resultCode] || '网络异常，请检查网络后重试'}(${json.data.resultCode})`)
    }
  })
})

var userPrivacyValidate = (params) => new Promise((resolve, reject) => {
  var body = `<?xml version='1.0' encoding='utf-8'?>
              <userPrivacyValidate>
                  <userPrivacyValidateReq>
                      <account>${window.userAccount}</account>
                  </userPrivacyValidateReq>
              </userPrivacyValidate>`
  transparentTransmission('bmsuite.IUser', body, null, 'userprivacyvalidate', json => {
    if (json.code === 'S_OK') {
      resolve(json)
    } else {
      toast(`${errcodeList['' + json.data.resultCode] || '网络异常，请检查网络后重试'}(${json.data.resultCode})`)
    }
  })
})
/* 子窗口调用主窗口函数 */
var post2window = (item) => {
  return new Promise((resolve, reject) => {
    jsonRequestMission("sendToOtherHtml", item, function (result) {
      console.log(result);
      resolve()
    });
    // 置顶
    jsonRequestMission("titleOperate", {
      operate: 9,
      userId: window.userAccount
    }, (res) => {
      console.log(res);
    })
  });
}

var appOpenPcDomApi = (item) => new Promise((resolve, reject) => {
  if (window && window.userDomainId && window.userDomainId !== '') {
    TransmissionAppOpenPcDomApiPds('interaction/queryFileData', item, 'encodeData', json => {
      if (json.code === 'S_OK') {
        resolve(json);
      } else {
        reject(json);
      }
    });
  } else {
    TransmissionAppOpenPcDomApi('interaction/queryFileData', item, 'encodeData', json => {
      if (json.code === 'S_OK') {
        resolve(json);
      } else {
        reject(json);
      }
    });
  }
});
var signatureAndAESencrypt = (num) => {
  let str = "";
  let uuid = getuuid();
  let timeStamp = new Date().getTime();
  if (num == 2) {
    if (!(window.dynamic_clientId || window.dynamic_clientKey)) {
      return;
    }
    str = `clientId=${window.dynamic_clientId}&clientKey=${window.dynamic_clientKey}&msgId=${uuid}&timeStamp=${timeStamp}&version=1.1`;
  } else if (num == 3) {
    if (!window.userAccount) {
      return;
    }
    str = window.userAccount;
  } else if (num == 6) {
    if (!window.userAccount) {
      return;
    }
    str = window.userAccount;
  }
  return new Promise((resolve, reject) => {
    jsonRequestMission("safeBoxEncrypt", {
      context: str,
      type: num
    }, function (result) {
      if (result.code === 'S_OK') {
        resolve({
          strr: result.var.ciphertext,
          uuidc: num == 2 ? uuid : "",
          timeStampc: num == 2 ? timeStamp : ""
        });
      } else {
        reject(result)
      }
    });
  });
};

var signatureAndAESencryptMmc = (num) => {
  let str = "";
  let uuid = getuuid();
  let timeStamp = new Date().getTime();
  if (num == 2) {
    if (!(window.mmc_clientId || window.mmc_clientKey)) {
      return;
    }
    str = `clientId=${window.mmc_clientId}&clientKey=${window.mmc_clientKey}&msgId=${uuid}&timeStamp=${timeStamp}&version=1.1`;
  } else if (num == 3) {
    if (!window.userAccount) {
      return;
    }
    str = window.userAccount;
  } else if (num == 6) {
    if (!window.userAccount) {
      return;
    }
    str = window.userAccount;
  }
  return new Promise((resolve, reject) => {
    jsonRequestMission("safeBoxEncrypt", {
      context: str,
      type: num
    }, function (result) {
      if (result.code === 'S_OK') {
        resolve({
          strr: result.var.ciphertext,
          uuidc: num == 2 ? uuid : "",
          timeStampc: num == 2 ? timeStamp : ""
        });
      } else {
        reject(result)
      }
    });
  });
};

function getuuid() {
  var d = new Date().getTime();
  if (window.performance && typeof window.performance.now === "function") {
    d += performance.now(); //use high-precision timer if available
  }
  var uuid = 'xxxxxxxxxxxx4xxxyxxxxxxxxxxxxxxx'.replace(/[xy]/g, function (c) {
    var r = (d + Math.random() * 16) % 16 | 0;
    d = Math.floor(d / 16);
    return (c == 'x' ? r : (r & 0x3 | 0x8)).toString(16);
  });
  return uuid;
};

function dynamicDeclare(id, type) {
  if (!id) return;
  Promise.all([signatureAndAESencrypt(2), signatureAndAESencrypt(3)]).then(res => {
    let code = {
      encodeData: res[1].strr,
      signature: res[0] && res[0].strr ? res[0].strr : "",
      uuidd: res[0] && res[0].uuidc ? res[0].uuidc : "",
      timeStampd: res[0] && res[0].timeStampc ? res[0].timeStampc : "",
    }
    uploadBrowseData({
      "encodeType": "2",
      "encodeData": code.encodeData,
      "terminalType": 5,
      "dynamicContentList": [{
        contentId: id,
        contentType: type
      }],
    }, res => {
      console.log(res); console.log("--(dynamicDeclare)---图片id:" + id + "--模块type:" + type)
    }, code);
  }).catch((res) => { });
}

function uploadBrowseData(data, callback, code) {
  // console.log(data,'===1')
  // console.log(code,'===2')
  var head;
  head = `transparentTransmission
  account:${window.userAccount}
  url:${window.dynamicServerUrl}/pDynamicInfo/uploadBrowseData
  POST /pDynamicInfo/uploadBrowseData HTTP/1.1
  Accept:application/xml
  Connection:keep-alive
  Content-Type:application/json;charset=utf-8
  clientId:pcapp
  signature:${code.signature}
  version:1.1
  timeStamp:${code.timeStampd}
  msgId:${code.uuidd}`


  let body = JSON.stringify(data);
  var content = head + "\n\n" + body;
  //请求次数
  ++window.timeIndex;
  ++window.timeIndex;
  window.QRCodeIndex.push(window.timeIndex);
  window.bridge.h5request(window.timeIndex, content);
  //记录保存请求函数对应callback
  window.resultList[window.timeIndex] = callback;
}

/**
     * @desc 查询视频线上播放地址
     * @date 2023-01-10 13:55:38
     * @param {Object} [item] - 视频信息
     * @param {Function} [callback] - 回调函数
     **/
function GZPgetFlvOnlineAddr(item, callback, isChace = true) {
  let moundDiskUrl = window.moundDiskUrl;
  if (!moundDiskUrl) {
    moundDiskUrl = getPublicUrlSi("mount", "cutoverInfoGZP", 'mountVideo');
  }
  let requestParams = {
    userId: window.userAccount,
    fileId: item.id,
    category: "video",
    expireSec: 14400, //过期时间最大14400s（4h）， 默认900s（15min）
  };
  writeLog(0, ("requestParams: " + JSON.stringify(requestParams) + ""));
  console.log("GZPgetFlvOnlineAddr入参", moundDiskUrl, requestParams);
  transparentTransmission("videoPreview/getPreviewInfo", JSON.stringify(requestParams), null, "videoPreview", (result) => {
    console.log("GZPgetFlvOnlineAddr响应~", result);
    if (result.code != "S_OK") {
      writeLog(1, "reault : " + result);
      toast("当前视频在线播放失败", { type: "info", delay: 3000 });
      return;
    }
    writeLog(1, "reault : " + result);
    if (result.data) {
      result.var = result.data;
    }
    callback(result);
  }, false, true, true, moundDiskUrl)
}

/**
     * @desc 查询视频线上播放地址
     * @date 2023-01-10 13:55:38
     * @param {Object} [item] - 视频信息
     * @param {Function} [callback] - 回调函数
     **/
function getFlvOnlineAddr(item, callback, isChace = true, sessionId = '') {
  window.personalCloudUrl = window.personalCloudUrl ? window.personalCloudUrl : getPublicUrlSi('personal') ? getPublicUrlSi('personal') : "";
  var userAccount = window.userAccount;

  if (personalCloudUrl) {
    let requestParams = {
      userId: window.userAccount,
      fileId: item.id,
      category: "video",
      expireSec: 14400, //过期时间最大14400s（4h）， 默认900s（15min）
    };
    // 保险箱打开视频播放，传sessionId
    if (sessionId) {
      requestParams.sessionId = sessionId;
    }
    writeLog(0, ("requestParams: " + JSON.stringify(requestParams) + ""));
    jsonRequestMission("getVideoInformation", requestParams, (result) => {
      if (result.code != "S_OK") {
        writeLog(1, "reault : " + result);
        toast("当前视频在线播放失败", { type: "info", delay: 3000 });
        return;
      }
      writeLog(1, "reault : " + result);
      callback(result);
    }, false, (personalCloudUrl ? 1 : 0), "getVideoInformation", false, true, personalCloudUrl)
  }
  else {
    var body = `<?xml version="1.0" encoding="utf-8"?> 
        <getFlvOnlineAddr> 
        <getFlvOnlineAddrReq>
            <contentID>${item.id}</contentID> 
            <ownerMSISDN>${userAccount}</ownerMSISDN>
        </getFlvOnlineAddrReq>
        </getFlvOnlineAddr>`

    transparentTransmission(
      "IContent",
      body,
      null,
      "getFlvOnlineAddr",
      (json) => {
        if (json.code === "S_OK") {
          callback(json)
        } else {
          //接口报错返回空
          if (json.data.resultCode == '200000764') {
            toast('视频内容过大，请稍后下载播放');
          } else {
            toast(
              `${errcodeList["" + json.data.resultCode] ||
              "网络异常，请检查网络后重试"}(${json.data.resultCode})`
            );
          }
        }
      },
      isChace
    );
  }
}
// 影视资源在线地址
function  movieGetFlvOnlineAddr(item, callback, isChace = true, sessionId = ''){
  
    let requestParams = {
      account: window.userAccount,
      fileId: item.id,
      category: "video",
      expireSec: 14400, //过期时间最大14400s（4h）， 默认900s（15min）
    };
    writeLog(0, ("requestParams: " + JSON.stringify(requestParams) + ""));
    let host = "https://orches.yun.139.com";
    let requestUrl = "orchestration/file/video/v1.0/queryFlvAddr";
    writeLog(0, ("requestParams: " + JSON.stringify(requestParams) + ""));
    requestParams = JSON.stringify(requestParams)
    transparentTransmission(requestUrl, requestParams, null, "queryFlvAddr", (result) => {
        if (result.code != "S_OK") 
        {
            writeLog(1, "reault : " + result);
            toast("当前视频在线播放失败", {type: "info", delay: 3000});
            return;
        }
        writeLog(1, "reault : " + result);
        callback(result);
    }, true, true, true, host)
  
}